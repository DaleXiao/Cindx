use crate::persistence_runtime::{
    app_data_root, append_startup_log, open_app_store, secure_directory, secure_private_file,
};
use crate::runtime_constants::{
    AGENT_HISTORY_MAX_TOOL_METADATA_BYTES, EVENT_REDACTION_MARKER_FILE,
    PERSISTED_TOOL_EVENT_METADATA_VALUE_LIMIT, PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES,
    TOOL_EVENT_METADATA_COMPACTION_MARKER_FILE,
};
use agent_core::{Event, Metadata};
use agent_storage::{SqliteStore, StorageError};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) fn compact_tool_event_metadata(metadata: &mut Metadata) -> usize {
    let oversized_keys = metadata
        .iter()
        .filter(|(_, value)| value.len() > PERSISTED_TOOL_EVENT_METADATA_VALUE_LIMIT)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();

    for key in &oversized_keys {
        let Some(value) = metadata.remove(key) else {
            continue;
        };
        let value_length = value.len();
        metadata
            .entry(format!("{key}_length"))
            .or_insert_with(|| value_length.to_string());
        metadata.insert(format!("{key}_omitted"), "true".to_string());
        let replacement = if key == "output" {
            truncate_utf8_bytes(&value, PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES)
        } else {
            format!("[omitted: {value_length}-byte tool metadata]")
        };
        metadata.insert(key.clone(), replacement);
    }

    oversized_keys.len()
}

pub(crate) fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

pub(crate) fn redact_metadata(metadata: &Metadata) -> Metadata {
    metadata
        .iter()
        .map(|(key, value)| {
            let redacted = if is_sensitive_assignment_key(key) {
                "[REDACTED]".to_string()
            } else if key == "memory_record_json" {
                serde_json::from_str::<agent_memory::MemoryRecord>(value)
                    .ok()
                    .filter(|record| {
                        serde_json::to_string(record).is_ok_and(|encoded| encoded == *value)
                            && !record.contains_sensitive_persisted_value()
                    })
                    .map(|_| value.clone())
                    .unwrap_or_else(|| {
                        redact_structured_json(value)
                            .unwrap_or_else(|| redact_sensitive_text(value))
                    })
            } else if key == "raw_tool_calls_json" {
                redact_structured_json(value).unwrap_or_else(|| redact_sensitive_text(value))
            } else {
                redact_sensitive_text(value)
            };
            (key.clone(), redacted)
        })
        .collect()
}

pub(crate) fn redact_structured_json(value: &str) -> Option<String> {
    let mut parsed = serde_json::from_str::<serde_json::Value>(value).ok()?;
    redact_json_value(&mut parsed);
    serde_json::to_string(&parsed).ok()
}

pub(crate) fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            values.iter_mut().for_each(redact_json_value);
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                if is_sensitive_assignment_key(key) {
                    *value = serde_json::Value::String("[REDACTED]".to_string());
                } else {
                    redact_json_value(value);
                }
            }
        }
        serde_json::Value::String(text) => {
            *text = redact_structured_json(text).unwrap_or_else(|| redact_sensitive_text(text));
        }
        _ => {}
    }
}

pub(crate) fn redact_event(mut event: Event) -> Event {
    event.summary = redact_sensitive_text(&event.summary);
    event.metadata = redact_metadata(&event.metadata);
    event
}

pub(crate) fn redact_persisted_events(store: &mut SqliteStore) -> Result<usize, StorageError> {
    let mut updated = 0;
    for event in store.list_all_events()? {
        let redacted = redact_event(event.clone());
        if redacted.summary != event.summary || redacted.metadata != event.metadata {
            store.update_event_content(&redacted)?;
            updated += 1;
        }
    }
    Ok(updated)
}

pub(crate) fn event_redaction_marker_path(root: &Path) -> PathBuf {
    root.join(EVENT_REDACTION_MARKER_FILE)
}

pub(crate) fn event_redaction_complete(root: &Path) -> bool {
    event_redaction_marker_path(root).is_file()
}

pub(crate) fn mark_event_redaction_complete(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("failed to create event redaction marker directory: {error}"))?;
    secure_directory(root)
        .map_err(|error| format!("failed to secure event redaction marker directory: {error}"))?;
    let path = event_redaction_marker_path(root);
    fs::write(&path, b"events-redaction-v1\n")
        .map_err(|error| format!("failed to write event redaction marker: {error}"))?;
    secure_private_file(&path)
        .map_err(|error| format!("failed to secure event redaction marker: {error}"))?;
    Ok(())
}

pub(crate) fn tool_event_metadata_compaction_marker_path(root: &Path) -> PathBuf {
    root.join(TOOL_EVENT_METADATA_COMPACTION_MARKER_FILE)
}

pub(crate) fn tool_event_metadata_compaction_complete(root: &Path) -> bool {
    tool_event_metadata_compaction_marker_path(root).is_file()
}

pub(crate) fn mark_tool_event_metadata_compaction_complete(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| {
        format!("failed to create tool metadata compaction marker directory: {error}")
    })?;
    secure_directory(root).map_err(|error| {
        format!("failed to secure tool metadata compaction marker directory: {error}")
    })?;
    let path = tool_event_metadata_compaction_marker_path(root);
    fs::write(&path, b"events-tool-metadata-v1\n")
        .map_err(|error| format!("failed to write tool metadata compaction marker: {error}"))?;
    secure_private_file(&path)
        .map_err(|error| format!("failed to secure tool metadata compaction marker: {error}"))?;
    Ok(())
}

pub(crate) fn compact_persisted_tool_event_metadata(
    store: &mut SqliteStore,
) -> Result<usize, StorageError> {
    let event_ids = store.oversized_tool_event_ids(AGENT_HISTORY_MAX_TOOL_METADATA_BYTES)?;
    let mut updated = 0;
    for event_id in event_ids {
        let Some(mut event) = store.event_by_id(&event_id)? else {
            continue;
        };
        if compact_tool_event_metadata(&mut event.metadata) == 0 {
            continue;
        }
        store.update_event_content(&event)?;
        updated += 1;
    }
    Ok(updated)
}

pub(crate) fn start_tool_event_metadata_compaction() {
    let root = app_data_root();
    if tool_event_metadata_compaction_complete(&root) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(Duration::from_secs(2));
        let result = open_app_store()
            .and_then(|mut store| compact_persisted_tool_event_metadata(&mut store));
        match result {
            Ok(updated) => {
                if let Err(error) = mark_tool_event_metadata_compaction_complete(&root) {
                    append_startup_log(&error);
                } else {
                    append_startup_log(&format!(
                        "compacted {updated} oversized tool event metadata records"
                    ));
                }
            }
            Err(error) => {
                append_startup_log(&format!("tool event metadata compaction failed: {error}"))
            }
        }
    });
}

pub(crate) fn redact_existing_text_artifact(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read text artifact: {error}"))?;
    let redacted = redact_sensitive_text(&text);
    if redacted == text {
        return Ok(false);
    }

    let mut options = fs::OpenOptions::new();
    options.truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("failed to open text artifact: {error}"))?;
    file.write_all(redacted.as_bytes())
        .map_err(|error| format!("failed to rewrite text artifact: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure text artifact: {error}"))?;
    Ok(true)
}

pub(crate) fn redact_sensitive_text(value: &str) -> String {
    value
        .split('\n')
        .map(redact_sensitive_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn redact_sensitive_line(line: &str) -> String {
    for (index, character) in line.char_indices() {
        if matches!(character, '=' | ':') && is_sensitive_assignment_key(&line[..index]) {
            return format!("{}[REDACTED]", &line[..=index]);
        }
    }

    let lower = line.to_ascii_lowercase();
    if let Some(index) = lower.find("bearer ") {
        return format!("{}Bearer [REDACTED]", &line[..index]);
    }

    [
        ("github_pat_", 20_usize),
        ("ghp_", 16_usize),
        ("xoxb-", 16_usize),
        ("sk-", 16_usize),
        ("AKIA", 16_usize),
    ]
    .into_iter()
    .fold(line.to_string(), |text, (prefix, minimum_length)| {
        redact_prefixed_secret(&text, prefix, minimum_length)
    })
}

pub(crate) fn is_sensitive_assignment_key(value: &str) -> bool {
    let compact = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "apikey",
        "xapikey",
        "authorization",
        "proxyauthorization",
        "accesstoken",
        "refreshtoken",
        "authtoken",
        "clientsecret",
        "secretkey",
        "password",
    ]
    .iter()
    .any(|key| compact.ends_with(key))
}

pub(crate) fn redact_prefixed_secret(value: &str, prefix: &str, minimum_length: usize) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_start) = value[cursor..].find(prefix) {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let mut end = start + prefix.len();
        while end < value.len() {
            let byte = value.as_bytes()[end];
            if byte.is_ascii_whitespace()
                || matches!(byte, b'\'' | b'"' | b',' | b';' | b')' | b']' | b'}')
            {
                break;
            }
            end += 1;
        }
        if end.saturating_sub(start) >= minimum_length {
            output.push_str("[REDACTED]");
        } else {
            output.push_str(prefix);
        }
        cursor = end;
    }
    output.push_str(&value[cursor..]);
    output
}
