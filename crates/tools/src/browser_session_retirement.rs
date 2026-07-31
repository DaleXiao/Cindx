use crate::{desktop_control::run_sidecar_controlled, resolve_workspace_path, ToolError};
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    path::{Component, Path},
    time::Duration,
};

const BROWSER_SESSION_RETIREMENT_SCHEMA: &str = "cindx.browser-session-retirement.v1";

pub fn retire_browser_session(
    workspace_root: &Path,
    session_relative: &Path,
) -> Result<(), ToolError> {
    let components = session_relative.components().collect::<Vec<_>>();
    let session_name = match components.as_slice() {
        [Component::Normal(cindx), Component::Normal(browser_sessions), Component::Normal(session)]
            if *cindx == ".cindx"
                && *browser_sessions == "browser-sessions"
                && !session.is_empty()
                && *session != ".retired" =>
        {
            *session
        }
        _ => {
            return Err(ToolError::new(
                "browser retirement path must be a direct .cindx/browser-sessions child",
            ))
        }
    };
    let session_relative = session_relative
        .to_str()
        .ok_or_else(|| ToolError::new("browser retirement path must be valid UTF-8"))?;
    let session_dir = resolve_workspace_path(workspace_root, session_relative)?;
    let source_exists =
        regular_directory_or_absent(&session_dir, "browser session retirement target")?;
    let sessions_root = session_dir
        .parent()
        .ok_or_else(|| ToolError::new("browser session retirement target has no parent"))?;
    let retirement_root = sessions_root.join(".retired");
    let retirement_root_exists =
        regular_directory_or_absent(&retirement_root, "browser session retirement quarantine")?;
    let retired_exists = if retirement_root_exists {
        regular_directory_or_absent(
            &retirement_root.join(session_name),
            "retired browser session",
        )?
    } else {
        false
    };
    if !source_exists && !retired_exists {
        return Ok(());
    }

    let arguments = [
        OsString::from("--retire-session"),
        session_dir.as_os_str().to_os_string(),
    ];
    let output = run_sidecar_controlled(
        "CINDX_BROWSER_SIDECAR",
        &arguments,
        &crate::ToolExecutionControl::never_cancelled(),
        Duration::from_secs(10),
    )?;
    if output.cancelled || output.timed_out {
        return Err(ToolError::new(
            "browser session retirement did not complete",
        ));
    }
    let response: Value = serde_json::from_str(&output.stdout)
        .map_err(|error| ToolError::new(format!("invalid browser retirement response: {error}")))?;
    if response.get("schema").and_then(Value::as_str) != Some(BROWSER_SESSION_RETIREMENT_SCHEMA)
        || response.get("retired").and_then(Value::as_bool) != Some(true)
    {
        return Err(ToolError::new(
            "browser sidecar returned an unsupported retirement response",
        ));
    }
    Ok(())
}

fn regular_directory_or_absent(path: &Path, label: &str) -> Result<bool, ToolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(ToolError::new(format!(
            "{label} is not a regular directory"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ToolError::new(format!(
            "failed to inspect {label}: {error}"
        ))),
    }
}
