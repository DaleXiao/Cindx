use crate::app_state::AppState;
use crate::configuration_models::{
    McpServerInput, McpServerPolicyInput, McpServerView, McpServersInput, McpStateView,
    SidecarConfig, SkillPackageInstallInput, SkillPreferenceInput, SkillStateView,
    SkillUrlInstallInput,
};
use crate::persistence_runtime::{
    active_workspace_root, invalidate_tool_registry_cache, skill_catalog_for_root,
};
use crate::runtime_values::normalized_config_value;
use crate::sidecar_runtime::{
    apply_sidecar_env, save_sidecar_config_to_disk, save_web_search_config_to_disk, sidecar_state,
};
use crate::view_models::{
    SidecarConfigInput, SidecarState, WebSearchConfigInput, WebSearchConfigState,
};
use agent_mcp::{McpCatalogService, McpTransportConfig};
use agent_skills::{install_skill_archive as install_skill_archive_package, SkillPreference};
use base64::Engine;
use std::path::Path;
use tauri::Manager;
use tools::WebSearchConfig;

/// The sidecar health probe spawns `sidecar --health` per configured endpoint and
/// waits for each to exit, which can cost as long as a cold node start. A sync
/// command would spend that on the IPC thread and stall the UI, so the body runs
/// on a blocking task.
#[tauri::command]
pub(crate) async fn get_sidecar_state(app: tauri::AppHandle) -> Result<SidecarState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        get_sidecar_state_blocking(state)
    })
    .await
    .map_err(|error| format!("sidecar state failed to join: {error}"))?
}

fn get_sidecar_state_blocking(state: tauri::State<'_, AppState>) -> Result<SidecarState, String> {
    let config = state
        .sidecar_config
        .lock()
        .map_err(|error| format!("sidecar config lock poisoned: {error}"))?
        .clone();
    Ok(sidecar_state(&config, None))
}

/// Saving the sidecar configuration writes it to disk and then health-probes both
/// endpoints, so it blocks for as long as `get_sidecar_state` does.
#[tauri::command]
pub(crate) async fn save_sidecar_config(
    app: tauri::AppHandle,
    input: SidecarConfigInput,
) -> Result<SidecarState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        save_sidecar_config_blocking(state, input)
    })
    .await
    .map_err(|error| format!("sidecar configuration save failed to join: {error}"))?
}

fn save_sidecar_config_blocking(
    state: tauri::State<'_, AppState>,
    input: SidecarConfigInput,
) -> Result<SidecarState, String> {
    let config = SidecarConfig {
        browser_path: normalized_config_value(&input.browser_path),
        computer_path: normalized_config_value(&input.computer_path),
        auto_configure: input.auto_configure,
    };
    save_sidecar_config_to_disk(&config).map_err(|error| error.to_string())?;
    apply_sidecar_env(&config);
    let mut stored = state
        .sidecar_config
        .lock()
        .map_err(|error| format!("sidecar config lock poisoned: {error}"))?;
    *stored = config.clone();
    Ok(sidecar_state(&config, None))
}

#[tauri::command]
pub(crate) fn get_web_search_config(
    state: tauri::State<'_, AppState>,
) -> Result<WebSearchConfigState, String> {
    let config = state
        .web_search_config
        .lock()
        .map_err(|error| format!("web search config lock poisoned: {error}"))?;
    Ok(web_search_config_state(&config))
}

#[tauri::command]
pub(crate) fn save_web_search_config(
    state: tauri::State<'_, AppState>,
    input: WebSearchConfigInput,
) -> Result<WebSearchConfigState, String> {
    let endpoint = normalized_config_value(&input.endpoint);
    let requested_api_key = normalized_config_value(&input.api_key);

    let mut config = state
        .web_search_config
        .lock()
        .map_err(|error| format!("web search config lock poisoned: {error}"))?;
    let effective_api_key = if endpoint.is_empty() {
        ""
    } else if requested_api_key.is_empty() {
        &config.api_key
    } else {
        &requested_api_key
    };
    validate_web_search_transport(&endpoint, effective_api_key)?;
    config.endpoint = endpoint;
    if config.endpoint.is_empty() {
        config.api_key.clear();
    } else if !requested_api_key.is_empty() {
        config.api_key = requested_api_key;
    }
    save_web_search_config_to_disk(&config).map_err(|error| error.to_string())?;
    invalidate_tool_registry_cache(&state)?;
    Ok(web_search_config_state(&config))
}

fn validate_web_search_transport(endpoint: &str, api_key: &str) -> Result<(), String> {
    if !(endpoint.is_empty() || endpoint.starts_with("https://") || endpoint.starts_with("http://"))
    {
        return Err("web search endpoint must start with http:// or https://".to_string());
    }
    if endpoint.starts_with("http://") && !api_key.trim().is_empty() {
        return Err("web search endpoints with an API key must use HTTPS".to_string());
    }
    Ok(())
}

pub(crate) fn web_search_config_state(config: &WebSearchConfig) -> WebSearchConfigState {
    WebSearchConfigState {
        endpoint: config.endpoint.clone(),
        api_key_set: !config.api_key.trim().is_empty(),
        configured: !config.endpoint.trim().is_empty(),
    }
}

#[tauri::command]
pub(crate) fn get_mcp_state(state: tauri::State<'_, AppState>) -> Result<McpStateView, String> {
    let catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
pub(crate) fn save_mcp_servers(
    state: tauri::State<'_, AppState>,
    input: McpServersInput,
) -> Result<McpStateView, String> {
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    catalog
        .save_servers(input.servers)
        .map_err(|error| error.to_string())?;
    invalidate_tool_registry_cache(&state)?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
pub(crate) fn upsert_mcp_server(
    state: tauri::State<'_, AppState>,
    input: McpServerInput,
) -> Result<McpStateView, String> {
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    catalog
        .upsert_server(input.server)
        .map_err(|error| error.to_string())?;
    invalidate_tool_registry_cache(&state)?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
pub(crate) fn import_external_mcp_servers(
    state: tauri::State<'_, AppState>,
) -> Result<McpStateView, String> {
    let workspace_root = active_workspace_root(&state).ok();
    let candidate_paths = agent_mcp::external_mcp_config_candidate_paths(workspace_root.as_deref());
    let mut incoming = Vec::new();
    for path in candidate_paths {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        incoming.extend(agent_mcp::parse_external_mcp_servers(&raw));
    }
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    let existing_names: std::collections::BTreeSet<String> = catalog
        .states()
        .into_iter()
        .map(|server| server.config.name)
        .collect();
    for server in incoming {
        if existing_names.contains(&server.name) {
            continue;
        }
        catalog
            .upsert_server(server)
            .map_err(|error| error.to_string())?;
    }
    invalidate_tool_registry_cache(&state)?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
pub(crate) fn update_mcp_server_policy(
    state: tauri::State<'_, AppState>,
    input: McpServerPolicyInput,
) -> Result<McpStateView, String> {
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    catalog
        .update_policy(&input.server_id, input.enabled, input.require_approval)
        .map_err(|error| error.to_string())?;
    invalidate_tool_registry_cache(&state)?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
pub(crate) fn remove_mcp_server(
    state: tauri::State<'_, AppState>,
    server_id: String,
) -> Result<McpStateView, String> {
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    catalog
        .remove_server(&server_id)
        .map_err(|error| error.to_string())?;
    invalidate_tool_registry_cache(&state)?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
pub(crate) async fn refresh_mcp_server(
    app: tauri::AppHandle,
    server_id: String,
) -> Result<McpStateView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut catalog = state
            .mcp_catalog
            .lock()
            .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
        let last_error = catalog
            .refresh_server(&server_id)
            .err()
            .map(|error| error.to_string());
        invalidate_tool_registry_cache(&state)?;
        Ok(mcp_state_view(&catalog, last_error))
    })
    .await
    .map_err(|error| format!("MCP refresh failed to join: {error}"))?
}

pub(crate) fn mcp_state_view(
    catalog: &McpCatalogService,
    last_error: Option<String>,
) -> McpStateView {
    let servers = catalog
        .states()
        .into_iter()
        .map(|state| {
            let (transport_type, command, args, url, secret_keys) = match &state.config.transport {
                McpTransportConfig::Stdio { command, args, env } => (
                    "stdio".to_string(),
                    Some(command.clone()),
                    args.clone(),
                    None,
                    env.keys().cloned().collect(),
                ),
                McpTransportConfig::StreamableHttp { url, headers } => (
                    "streamable_http".to_string(),
                    None,
                    Vec::new(),
                    Some(url.clone()),
                    headers.keys().cloned().collect(),
                ),
            };
            McpServerView {
                id: state.config.id,
                name: state.config.name,
                enabled: state.config.enabled,
                require_approval: state.config.require_approval,
                timeout_ms: state.config.timeout_ms,
                transport_type,
                command,
                args,
                url,
                secret_keys,
                tool_count: state.tool_count,
                refreshed_at_ms: state.refreshed_at_ms,
                last_error: state.last_error,
            }
        })
        .collect();
    McpStateView {
        servers,
        last_error,
    }
}

/// Loads the skill catalog from two filesystem roots on every call.
///
/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole read (P2-05).
#[tauri::command]
pub(crate) async fn get_skill_state(app: tauri::AppHandle) -> Result<SkillStateView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        get_skill_state_blocking(state)
    })
    .await
    .map_err(|error| format!("skill state failed to join: {error}"))?
}

fn get_skill_state_blocking(state: tauri::State<'_, AppState>) -> Result<SkillStateView, String> {
    let root = active_workspace_root(&state)?;
    let catalog = skill_catalog_for_root(&root);
    Ok(SkillStateView {
        skills: catalog.list(),
        last_error: None,
    })
}

#[tauri::command]
pub(crate) fn refresh_skills(state: tauri::State<'_, AppState>) -> Result<SkillStateView, String> {
    let root = active_workspace_root(&state)?;
    let catalog = skill_catalog_for_root(&root);
    let skills = catalog.refresh()?;
    invalidate_tool_registry_cache(&state)?;
    Ok(SkillStateView {
        skills,
        last_error: None,
    })
}

#[tauri::command]
pub(crate) fn save_skill_preference(
    state: tauri::State<'_, AppState>,
    input: SkillPreferenceInput,
) -> Result<SkillStateView, String> {
    let root = active_workspace_root(&state)?;
    let catalog = skill_catalog_for_root(&root);
    let skills = catalog.set_preference(
        &input.skill_id,
        SkillPreference {
            enabled: input.enabled,
            trusted: input.trusted,
        },
    )?;
    invalidate_tool_registry_cache(&state)?;
    Ok(SkillStateView {
        skills,
        last_error: None,
    })
}

#[tauri::command]
pub(crate) fn install_skill_package(
    state: tauri::State<'_, AppState>,
    input: SkillPackageInstallInput,
) -> Result<SkillStateView, String> {
    let root = active_workspace_root(&state)?;
    let bytes = decode_data_url(&input.data_base64)?;
    let skill_id = install_skill_archive_package(&root.join(".cindx/skills"), &bytes)?;
    let view = skill_state_after_install(&root, &skill_id)?;
    invalidate_tool_registry_cache(&state)?;
    Ok(view)
}

/// The complete `curl` argv for one skill-package download. The shared HTTP
/// policy owner supplies the bounded redirect count (previously curl's unlimited
/// default), the HTTPS-only scheme allowlist for the request and any redirect,
/// the bounded timeout, and the product user agent; this adds the failure and
/// package-size limits and the URL last.
pub(crate) fn skill_download_curl_args(url: &str) -> Vec<String> {
    let policy = agent_core::http_policy(agent_core::HttpEgressProfile::SkillInstall);
    let mut argv = agent_core::curl_policy_args(&policy);
    argv.extend([
        "--fail".to_string(),
        "--max-filesize".to_string(),
        policy.response_max_bytes.to_string(),
        url.to_string(),
    ]);
    argv
}

#[tauri::command]
pub(crate) fn install_skill_url(
    state: tauri::State<'_, AppState>,
    input: SkillUrlInstallInput,
) -> Result<SkillStateView, String> {
    let url = input.url.trim();
    if !url.starts_with("https://") {
        return Err("skill URL must use HTTPS".to_string());
    }
    let output = std::process::Command::new("/usr/bin/curl")
        .args(skill_download_curl_args(url))
        .output()
        .map_err(|error| format!("failed to download skill: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            "skill download failed".to_string()
        } else {
            format!("skill download failed: {detail}")
        });
    }
    let root = active_workspace_root(&state)?;
    let skill_id = install_skill_archive_package(&root.join(".cindx/skills"), &output.stdout)?;
    let view = skill_state_after_install(&root, &skill_id)?;
    invalidate_tool_registry_cache(&state)?;
    Ok(view)
}

pub(crate) fn decode_data_url(value: &str) -> Result<Vec<u8>, String> {
    let encoded = value.split_once(',').map(|(_, data)| data).unwrap_or(value);
    if encoded.len() > 70 * 1024 * 1024 {
        return Err("skill data exceeds the 50 MB limit".to_string());
    }
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("skill data is not valid base64: {error}"))
}

pub(crate) fn skill_state_after_install(
    root: &Path,
    skill_id: &str,
) -> Result<SkillStateView, String> {
    let catalog = skill_catalog_for_root(root);
    catalog.refresh()?;
    let skills = catalog.set_preference(
        skill_id,
        SkillPreference {
            enabled: false,
            trusted: false,
        },
    )?;
    Ok(SkillStateView {
        skills,
        last_error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{skill_download_curl_args, validate_web_search_transport};

    #[test]
    fn web_search_transport_requires_https_only_when_an_api_key_is_present() {
        assert!(validate_web_search_transport("https://search.example.test", "secret").is_ok());
        assert!(validate_web_search_transport("http://127.0.0.1:8080", "").is_ok());
        assert!(
            validate_web_search_transport("http://127.0.0.1:8080", "secret")
                .unwrap_err()
                .contains("must use HTTPS")
        );
    }

    /// The skill download takes its posture from the shared HTTP policy owner:
    /// bounded redirects instead of curl's unlimited default, HTTPS for the
    /// request and every redirect, the product user agent, and a stated size cap.
    #[test]
    fn skill_download_argv_is_bounded_by_the_shared_http_policy() {
        let argv = skill_download_curl_args("https://skills.example/package.zip");

        assert_eq!(argv[0], "-q");
        assert!(argv.contains(&"--silent".to_string()));
        assert!(argv.contains(&"--show-error".to_string()));
        assert!(argv.contains(&agent_core::HTTP_USER_AGENT.to_string()));
        assert!(argv.contains(&"--fail".to_string()));
        assert!(argv.contains(&"-L".to_string()));
        let bound = argv
            .iter()
            .position(|arg| arg == "--max-redirs")
            .expect("redirects are bounded");
        assert_eq!(
            argv[bound + 1],
            agent_core::HTTP_PUBLIC_FETCH_MAX_REDIRECTS.to_string()
        );
        assert_eq!(
            argv.iter().filter(|arg| arg.as_str() == "=https").count(),
            2,
            "the request and any redirect stay on HTTPS"
        );
        let size = argv
            .iter()
            .position(|arg| arg == "--max-filesize")
            .expect("size cap present");
        assert_eq!(
            argv[size + 1],
            agent_core::HTTP_SKILL_PACKAGE_MAX_BYTES.to_string()
        );
        assert!(argv.contains(&"30".to_string()), "argv was {argv:?}");
        assert_eq!(
            argv.last().map(String::as_str),
            Some("https://skills.example/package.zip")
        );
    }
}
