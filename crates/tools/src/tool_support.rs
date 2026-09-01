use crate::ToolError;
use agent_core::{
    Metadata, PermissionRequest, PermissionRequestId, PermissionRisk, TaskId, ToolCallId,
    ToolObservationV2, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn builtin_tool_spec(
    name: &str,
    description: &str,
    risk: ToolRisk,
    legacy_schema: &str,
) -> ToolSpec {
    let namespace = name.split('.').next().unwrap_or("builtin");
    ToolSpec::builtin(
        name,
        namespace,
        description,
        risk,
        object_schema_from_fields(legacy_schema),
    )
}

fn object_schema_from_fields(fields: &str) -> String {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for row in fields.lines() {
        let Some((name, descriptor)) = row.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let description = descriptor
            .trim()
            .trim_matches(|character| character == '<' || character == '>');
        if name.is_empty() {
            continue;
        }
        let value_type = match name {
            "destructive" | "new_tab" | "headless" | "full_page" | "exact" | "download"
            | "append" | "press_enter" => "boolean",
            "x" | "y" | "delta_x" | "delta_y" | "limit" | "max_results" | "timeout_ms" => "integer",
            _ => "string",
        };
        properties.insert(
            name.to_string(),
            serde_json::json!({
                "type": value_type,
                "description": description,
            }),
        );
        if !description.to_ascii_lowercase().contains("optional") {
            required.push(serde_json::Value::String(name.to_string()));
        }
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
    .to_string()
}

pub fn parse_input(input: &str) -> BTreeMap<String, String> {
    if let Ok(serde_json::Value::Object(values)) = serde_json::from_str(input) {
        if values.len() == 1 {
            if let Some(serde_json::Value::String(legacy)) = values.get("input") {
                return parse_input(legacy);
            }
        }
        return values
            .into_iter()
            .map(|(key, value)| {
                let value = match value {
                    serde_json::Value::String(value) => value,
                    other => other.to_string(),
                };
                (key, value)
            })
            .collect();
    }

    let mut parsed = BTreeMap::new();
    let mut current_key: Option<String> = None;
    for line in input.lines() {
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            current_key = Some(key.clone());
            parsed.insert(key, value.to_string());
        } else if let Some(key) = current_key.as_ref() {
            if let Some(value) = parsed.get_mut(key) {
                value.push('\n');
                value.push_str(line);
            }
        }
    }
    parsed
}

pub fn encode_input(entries: &[(&str, &str)]) -> String {
    entries
        .iter()
        .map(|(key, value)| format!("{key}={}", value.replace('\r', "")))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn required_input(
    input: &BTreeMap<String, String>,
    key: &str,
) -> Result<String, ToolError> {
    input
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| ToolError::new(format!("missing required input: {key}")))
}

pub(crate) fn parse_bounded_usize_input(
    input: &BTreeMap<String, String>,
    key: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, ToolError> {
    let value = match input.get(key) {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| ToolError::new(format!("{key} must be an integer")))?,
        None => default,
    };
    if value < minimum || value > maximum {
        return Err(ToolError::new(format!(
            "{key} must be between {minimum} and {maximum}"
        )));
    }
    Ok(value)
}

pub(crate) fn permission_request(
    task_id: &TaskId,
    risk: PermissionRisk,
    action: &str,
    reason: &str,
    scope: &str,
    metadata: Metadata,
) -> PermissionRequest {
    PermissionRequest {
        id: PermissionRequestId(format!(
            "perm-{}",
            stable_hash(&format!("{action}:{scope:?}"))
        )),
        task_id: task_id.clone(),
        risk,
        action: action.to_string(),
        reason: reason.to_string(),
        scope: scope.to_string(),
        metadata,
    }
}

pub(crate) fn tool_result(
    invocation_id: ToolCallId,
    status: ToolOutcomeStatus,
    output: String,
    metadata: Metadata,
) -> ToolResult {
    ToolResult::text(invocation_id, status, output, metadata)
}

pub(crate) fn model_observation(
    tool_name: &str,
    summary: impl Into<String>,
    evidence: impl Into<String>,
    evidence_complete: bool,
    facts: Metadata,
    next_action: Option<String>,
) -> ToolObservationV2 {
    let observation =
        ToolObservationV2::new(tool_name, summary, evidence, evidence_complete, facts);
    match next_action {
        Some(next_action) => observation.with_next_action(next_action),
        None => observation,
    }
}

pub(crate) fn bounded_model_text(value: &str, max_chars: usize) -> (String, bool) {
    const MARKER: &str = "\n...[middle evidence omitted]...\n";
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return (value.to_string(), false);
    }
    let marker_count = MARKER.chars().count();
    let retained = max_chars.saturating_sub(marker_count);
    let tail_count = retained / 4;
    let head_count = retained.saturating_sub(tail_count);
    let head = value.chars().take(head_count).collect::<String>();
    let tail = value
        .chars()
        .skip(character_count.saturating_sub(tail_count))
        .collect::<String>();
    (format!("{head}{MARKER}{tail}"), true)
}

pub(crate) fn resolve_workspace_path(
    workspace_root: &Path,
    path: &str,
) -> Result<PathBuf, ToolError> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(ToolError::new("absolute paths are not allowed"));
    }

    for component in candidate.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return Err(ToolError::new("path escapes the workspace"));
        }
    }

    let canonical_root = fs::canonicalize(workspace_root)
        .map_err(|error| ToolError::new(format!("failed to resolve workspace: {error}")))?;
    let resolved = workspace_root.join(candidate);
    let mut existing_ancestor = resolved.as_path();
    let canonical_ancestor = loop {
        match fs::symlink_metadata(existing_ancestor) {
            Ok(_) => {
                break fs::canonicalize(existing_ancestor).map_err(|error| {
                    ToolError::new(format!("failed to resolve workspace path: {error}"))
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing_ancestor = existing_ancestor
                    .parent()
                    .ok_or_else(|| ToolError::new("path escapes the workspace"))?;
            }
            Err(error) => {
                return Err(ToolError::new(format!(
                    "failed to inspect workspace path: {error}"
                )));
            }
        }
    };
    if !canonical_ancestor.starts_with(&canonical_root) {
        return Err(ToolError::new(
            "path escapes the workspace through a symbolic link",
        ));
    }

    Ok(resolved)
}

pub(crate) fn resolve_workspace_read_path(
    workspace_root: &Path,
    path: &Path,
) -> Result<PathBuf, ToolError> {
    let canonical_root = fs::canonicalize(workspace_root)
        .map_err(|error| ToolError::new(format!("failed to resolve workspace: {error}")))?;
    let canonical_path = fs::canonicalize(path)
        .map_err(|error| ToolError::new(format!("failed to resolve read path: {error}")))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ToolError::new(
            "path escapes the workspace through a symbolic link",
        ));
    }
    Ok(canonical_path)
}

pub(crate) fn input_value_is_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y"
    )
}

pub(crate) fn required_url(input: &BTreeMap<String, String>) -> Result<String, ToolError> {
    let url = required_input(input, "url")?;
    let normalized = url.trim().to_string();
    if normalized.contains('\n') || normalized.contains('\r') {
        return Err(ToolError::new("url must be a single line"));
    }
    if !(normalized.starts_with("https://") || normalized.starts_with("http://")) {
        return Err(ToolError::new("url must start with http:// or https://"));
    }

    Ok(normalized)
}

pub(crate) fn json_field(key: &str, value: &str) -> String {
    format!("\"{}\":\"{}\"", json_escape(key), json_escape(value))
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other if other.is_control() => escaped.push_str(&format!("\\u{:04x}", other as u32)),
            other => escaped.push(other),
        }
    }
    escaped
}

pub(crate) fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_millis() as u64
}

pub(crate) fn stable_hash(value: &str) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

/// Create the managed shell-artifact directory with owner-only access. The
/// directory holds per-command stdout/stderr captures, which may contain
/// secrets a command printed; world-readable defaults are unacceptable on a
/// shared machine.
pub(crate) fn private_dir_ensure(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Create (or truncate) a shell-artifact file that only the owner may read
/// or write. The permission force also covers pre-existing files, whose mode
/// a plain create-open would leave untouched.
pub(crate) fn private_file_create(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}
