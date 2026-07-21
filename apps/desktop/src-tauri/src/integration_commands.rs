use super::*;

#[tauri::command]
pub(crate) fn get_sidecar_state(state: tauri::State<'_, AppState>) -> Result<SidecarState, String> {
    let config = state
        .sidecar_config
        .lock()
        .map_err(|error| format!("sidecar config lock poisoned: {error}"))?
        .clone();
    Ok(sidecar_state(&config, None))
}

#[tauri::command]
pub(crate) fn save_sidecar_config(
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
    if !(endpoint.is_empty() || endpoint.starts_with("https://") || endpoint.starts_with("http://"))
    {
        return Err("web search endpoint must start with http:// or https://".to_string());
    }

    let mut config = state
        .web_search_config
        .lock()
        .map_err(|error| format!("web search config lock poisoned: {error}"))?;
    config.endpoint = endpoint;
    if config.endpoint.is_empty() {
        config.api_key.clear();
    } else {
        let api_key = normalized_config_value(&input.api_key);
        if !api_key.is_empty() {
            config.api_key = api_key;
        }
    }
    save_web_search_config_to_disk(&config).map_err(|error| error.to_string())?;
    invalidate_tool_registry_cache(&state)?;
    Ok(web_search_config_state(&config))
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

#[tauri::command]
pub(crate) fn get_skill_state(state: tauri::State<'_, AppState>) -> Result<SkillStateView, String> {
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
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-time",
            "30",
            "--max-filesize",
            "52428800",
            url,
        ])
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
