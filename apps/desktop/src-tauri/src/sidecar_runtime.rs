use crate::{
    configuration_models::SidecarConfig,
    persistence_runtime::{sidecar_config_path, web_search_config_path},
    runtime_values::sanitize_config_value,
    view_models::{SidecarEndpointState, SidecarState},
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tools::WebSearchConfig;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub(crate) fn load_sidecar_config() -> SidecarConfig {
    let mut config = SidecarConfig::default();
    let Ok(text) = fs::read_to_string(sidecar_config_path()) else {
        return config;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "browser_path" => config.browser_path = value.to_string(),
            "computer_path" => config.computer_path = value.to_string(),
            "auto_configure" => config.auto_configure = config_bool(value),
            _ => {}
        }
    }

    if config.browser_path.trim().is_empty() {
        config.browser_path = default_browser_sidecar_path().display().to_string();
    }
    if config.computer_path.trim().is_empty() {
        config.computer_path = default_computer_sidecar_path().display().to_string();
    }

    config
}

pub(crate) fn save_sidecar_config_to_disk(config: &SidecarConfig) -> Result<(), std::io::Error> {
    let path = sidecar_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(
        format!(
            "browser_path={}\ncomputer_path={}\nauto_configure={}\n",
            sanitize_config_value(&config.browser_path),
            sanitize_config_value(&config.computer_path),
            config.auto_configure
        )
        .as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

pub(crate) fn load_web_search_config() -> WebSearchConfig {
    let mut config = WebSearchConfig::default();
    let Ok(text) = fs::read_to_string(web_search_config_path()) else {
        return config;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "endpoint" => config.endpoint = value.to_string(),
            "api_key" => config.api_key = value.to_string(),
            _ => {}
        }
    }
    config
}

pub(crate) fn save_web_search_config_to_disk(
    config: &WebSearchConfig,
) -> Result<(), std::io::Error> {
    let path = web_search_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(
        format!(
            "endpoint={}\napi_key={}\n",
            sanitize_config_value(&config.endpoint),
            sanitize_config_value(&config.api_key)
        )
        .as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub(crate) fn apply_sidecar_env(config: &SidecarConfig) {
    if config.auto_configure {
        std::env::set_var("CINDX_BROWSER_SIDECAR", &config.browser_path);
        std::env::set_var("CINDX_COMPUTER_SIDECAR", &config.computer_path);
        if let Some(node_path) = find_node_executable() {
            std::env::set_var("CINDX_NODE", node_path);
        }
    } else {
        std::env::remove_var("CINDX_BROWSER_SIDECAR");
        std::env::remove_var("CINDX_COMPUTER_SIDECAR");
        std::env::remove_var("CINDX_NODE");
    }
}

pub(crate) fn sidecar_state(config: &SidecarConfig, last_error: Option<String>) -> SidecarState {
    SidecarState {
        browser: sidecar_endpoint_state("CINDX_BROWSER_SIDECAR", &config.browser_path),
        computer: sidecar_endpoint_state("CINDX_COMPUTER_SIDECAR", &config.computer_path),
        auto_configure: config.auto_configure,
        last_error,
    }
}

pub(crate) fn sidecar_endpoint_state(env_key: &str, path: &str) -> SidecarEndpointState {
    let path_buf = PathBuf::from(path);
    let exists = path_buf.exists();
    let executable = exists && path_buf.is_file();
    let health = if executable {
        let mut command = sidecar_command(&path_buf);
        command
            .arg("--health")
            .output()
            .map(|output| {
                (
                    output.status.success(),
                    if output.status.success() {
                        String::from_utf8_lossy(&output.stdout).trim().to_string()
                    } else {
                        String::from_utf8_lossy(&output.stderr).trim().to_string()
                    },
                )
            })
            .unwrap_or_else(|error| (false, error.to_string()))
    } else {
        (false, "sidecar path is missing".to_string())
    };

    SidecarEndpointState {
        path: path.to_string(),
        exists,
        executable,
        healthy: health.0,
        health_output: health.1,
        env_key: env_key.to_string(),
    }
}

pub(crate) fn sidecar_command(path: &Path) -> std::process::Command {
    if path.extension().and_then(|extension| extension.to_str()) == Some("js") {
        let mut command = std::process::Command::new(
            find_node_executable().unwrap_or_else(|| PathBuf::from("node")),
        );
        command.arg(path);
        command
    } else {
        std::process::Command::new(path)
    }
}

pub(crate) fn find_node_executable() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CINDX_NODE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    [
        PathBuf::from("/opt/homebrew/bin/node"),
        PathBuf::from("/usr/local/bin/node"),
        PathBuf::from("/usr/bin/node"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

pub(crate) fn default_browser_sidecar_path() -> PathBuf {
    bundled_or_development_resource("browser-sidecar.js")
}

pub(crate) fn default_computer_sidecar_path() -> PathBuf {
    bundled_or_development_resource("computer-sidecar.js")
}

pub(crate) fn development_repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

pub(crate) fn bundled_or_development_resource(file_name: &str) -> PathBuf {
    let packaged = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .and_then(|macos| macos.parent().map(Path::to_path_buf))
        .map(|contents| contents.join("Resources").join("sidecars").join(file_name));
    if let Some(path) = packaged.as_ref().filter(|path| path.is_file()) {
        return path.clone();
    }

    let development = development_repo_root()
        .join("scripts")
        .join("sidecars")
        .join(file_name);
    if development.is_file() {
        return development;
    }

    packaged.unwrap_or(development)
}

pub(crate) fn config_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y"
    )
}
