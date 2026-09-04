use super::*;
use crate::integration_commands;

const PERSISTENT_STORE_STARTUP_FAILURE: &str = "persistent state unavailable; startup aborted";

fn install_rustls_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    debug_assert!(rustls::crypto::CryptoProvider::get_default().is_some());
}

fn startup_probe_requested() -> bool {
    std::env::var("CINDX_STARTUP_PROBE")
        .map(|value| config_bool(&value))
        .unwrap_or(false)
}

fn persistent_store_startup_error(error: &StorageError) -> String {
    format!("{PERSISTENT_STORE_STARTUP_FAILURE}: {error}")
}

pub(crate) fn application_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

pub fn run() -> Result<(), String> {
    install_startup_panic_log();
    install_rustls_crypto_provider();
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
    let mut store = match open_app_store() {
        Ok(store) => store,
        Err(error) => {
            let message = persistent_store_startup_error(&error);
            append_startup_log(&format!("fatal startup: {message}"));
            if !startup_probe_requested() {
                show_native_startup_failure(&message);
            }
            return Err(message);
        }
    };
    if event_redaction_pending {
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
    let mut composition = match load_desktop_composition(&mut store) {
        Ok(composition) => composition,
        Err(message) => {
            append_startup_log(&format!("fatal startup: {message}"));
            if !startup_probe_requested() {
                show_native_startup_failure(&message);
            }
            return Err(message);
        }
    };
    if startup_probe_requested() {
        append_startup_log(&format!(
            "startup probe completed (source revision: {})",
            option_env!("CINDX_SOURCE_REVISION").unwrap_or("unknown")
        ));
        return Ok(());
    }

    let recovered_memory_refreshes = std::mem::take(&mut composition.recovered_memory_refreshes);
    let lifecycle_refresh_provider_config = composition.provider_config.clone();
    let app = tauri::Builder::default()
        .manage(compose_app_state(composition, store))
        .setup(move |app| {
            resume_recovered_memory_refreshes(
                recovered_memory_refreshes,
                &lifecycle_refresh_provider_config,
            );
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = install_macos_sidebar_material(&window) {
                    append_startup_log(&error);
                }
            }
            schedule_main_window_reveal_fallback(app.handle().clone());
            start_background_workers(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::Resized(_)
                    | tauri::WindowEvent::ScaleFactorChanged { .. }
                    | tauri::WindowEvent::Focused(_)
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
            report_frontend_crash,
            set_sidebar_material_width,
            get_runtime_status,
            integration_commands::get_sidecar_state,
            integration_commands::save_sidecar_config,
            integration_commands::get_web_search_config,
            integration_commands::save_web_search_config,
            integration_commands::get_mcp_state,
            integration_commands::save_mcp_servers,
            integration_commands::upsert_mcp_server,
            integration_commands::import_external_mcp_servers,
            integration_commands::update_mcp_server_policy,
            integration_commands::remove_mcp_server,
            integration_commands::refresh_mcp_server,
            integration_commands::get_skill_state,
            integration_commands::refresh_skills,
            integration_commands::save_skill_preference,
            integration_commands::install_skill_package,
            integration_commands::install_skill_url,
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
            set_session_model,
            sandbox_mode_runtime::set_session_sandbox_mode,
            sandbox_mode_runtime::get_session_sandbox_mode,
            generate_session_title,
            stage_agent_attachment,
            abort_agent_attachment_batch,
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
            model_supports_thinking,
            get_personalization_config,
            save_personalization_config,
            save_provider_config,
            list_provider_models,
            validate_image_endpoint,
            negotiate_voice_session,
            transcribe_voice_audio,
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
            resolve_agent_plan_confirmation,
            get_phase5_state,
            run_tool,
            resolve_tool_permission,
            get_phase7_state,
            memory_management_runtime::get_project_memory_state,
            memory_management_runtime::update_project_memory,
            workspace_undo_runtime::get_workspace_undo_state,
            workspace_undo_runtime::undo_workspace_change,
            workspace_undo_runtime::redo_workspace_change,
            custom_commands_runtime::get_custom_commands,
            ensure_workspace_knowledge,
            index_workspace_rag,
            search_rag,
            answer_with_rag,
            cancel_rag_operation,
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
        .build(application_context())
        .expect("error while building Cindx desktop app");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if !confirm_application_exit(app_handle) {
                api.prevent_exit();
            }
        }
    });
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QuitConfirmation {
    pub(crate) confirmed: bool,
    pub(crate) suppress_future: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        install_rustls_crypto_provider, persistent_store_startup_error,
        PERSISTENT_STORE_STARTUP_FAILURE,
    };
    use agent_storage::StorageError;

    #[test]
    fn rustls_crypto_provider_installation_is_idempotent() {
        install_rustls_crypto_provider();
        install_rustls_crypto_provider();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
        let _ = rustls::ClientConfig::builder();
    }

    #[test]
    fn persistent_store_startup_failure_preserves_the_cause() {
        let message = persistent_store_startup_error(&StorageError::new("database path is busy"));

        assert!(message.starts_with(PERSISTENT_STORE_STARTUP_FAILURE));
        assert!(message.contains("database path is busy"));
    }
}
