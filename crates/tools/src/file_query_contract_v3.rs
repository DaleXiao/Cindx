use super::{bounded_model_text, tool_result, ToolError};
use agent_core::{
    Metadata, ToolCallId, ToolObservationV2, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};

const MODEL_SEARCH_EVIDENCE_CHARS: usize = 5_200;
const SEARCH_CURSOR_MAX_BYTES: usize = 4 * 1024;
const SEARCH_CURSOR_PREFIX: &str = "cfs2_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchContextLine {
    pub(crate) line: usize,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchMatch {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) byte_offset: usize,
    pub(crate) text: String,
    pub(crate) before: Vec<SearchContextLine>,
    pub(crate) after: Vec<SearchContextLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchOptions {
    pub(crate) query: String,
    pub(crate) path: String,
    pub(crate) regex: bool,
    pub(crate) case_sensitive: bool,
    pub(crate) glob: Option<String>,
    pub(crate) context_lines: usize,
    pub(crate) max_results: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchCursor {
    pub(crate) next_path: String,
    pub(crate) next_line: usize,
    pub(crate) next_byte: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SearchCoverage {
    pub(crate) scanned_files: usize,
    pub(crate) scanned_bytes: u64,
    pub(crate) skipped_hidden_entries: usize,
    pub(crate) skipped_symlinks: usize,
    pub(crate) skipped_glob_files: usize,
    pub(crate) unreadable_files: usize,
    pub(crate) truncated_files: usize,
    pub(crate) discovery_limit_reached: bool,
    pub(crate) file_limit_reached: bool,
    pub(crate) byte_limit_reached: bool,
}

pub(crate) fn file_search_spec() -> ToolSpec {
    ToolSpec::builtin(
        "file.search",
        "file",
        "Search visible, non-symlink UTF-8 workspace files with bounded, deterministic pagination. Defaults preserve literal, case-sensitive, zero-context search.",
        ToolRisk::ReadOnly,
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "description": "Literal text or regular expression to find." },
                "path": { "type": "string", "default": ".", "description": "Optional workspace-relative file or directory." },
                "regex": { "type": "boolean", "default": false, "description": "Interpret query as a Rust regular expression." },
                "case_sensitive": { "type": "boolean", "default": true, "description": "Apply case-sensitive query and glob matching." },
                "glob": { "type": "string", "minLength": 1, "description": "Optional workspace-relative path glob." },
                "context_lines": { "type": "integer", "minimum": 0, "maximum": 8, "default": 0 },
                "max_results": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 },
                "cursor": { "type": "string", "minLength": 1, "description": "Opaque cursor returned by a prior call with identical options." }
            },
            "required": ["query"],
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": { "const": "cindx.file-search-result.v2" },
                "query": { "type": "string" },
                "path": { "type": "string" },
                "regex": { "type": "boolean" },
                "case_sensitive": { "type": "boolean" },
                "glob": { "type": ["string", "null"] },
                "context_lines": { "type": "integer", "minimum": 0 },
                "matches": { "type": "integer", "minimum": 0 },
                "results": { "type": "array" },
                "next_cursor": { "type": ["string", "null"] },
                "result_limit_reached": { "type": "boolean" },
                "complete": { "type": "boolean" },
                "scanned_files": { "type": "integer", "minimum": 0 },
                "scanned_bytes": { "type": "integer", "minimum": 0 },
                "skipped_hidden_entries": { "type": "integer", "minimum": 0 },
                "skipped_symlinks": { "type": "integer", "minimum": 0 },
                "skipped_glob_files": { "type": "integer", "minimum": 0 },
                "unreadable_files": { "type": "integer", "minimum": 0 },
                "truncated_files": { "type": "integer", "minimum": 0 },
                "discovery_limit_reached": { "type": "boolean" },
                "file_limit_reached": { "type": "boolean" },
                "byte_limit_reached": { "type": "boolean" }
            },
            "required": ["schema", "query", "path", "regex", "case_sensitive", "glob", "context_lines", "matches", "results", "next_cursor", "result_limit_reached", "complete", "scanned_files", "scanned_bytes", "skipped_hidden_entries", "skipped_symlinks", "skipped_glob_files", "unreadable_files", "truncated_files", "discovery_limit_reached", "file_limit_reached", "byte_limit_reached"],
            "additionalProperties": false
        })
        .to_string(),
    )
}

pub(crate) fn file_search_result(
    invocation_id: ToolCallId,
    options: SearchOptions,
    matches: Vec<SearchMatch>,
    coverage: SearchCoverage,
    next_cursor: Option<String>,
    cancelled: bool,
) -> ToolResult {
    let result_limit_reached = matches.len() >= options.max_results;
    let complete = !cancelled
        && next_cursor.is_none()
        && coverage.unreadable_files == 0
        && coverage.truncated_files == 0
        && !coverage.discovery_limit_reached
        && !coverage.file_limit_reached
        && !coverage.byte_limit_reached;
    let output = if cancelled {
        "File search cancelled.".to_string()
    } else {
        render_matches(&matches, options.context_lines)
    };
    let (evidence, evidence_truncated) = bounded_model_text(&output, MODEL_SEARCH_EVIDENCE_CHARS);
    let mut metadata = Metadata::from([
        ("query".to_string(), options.query.clone()),
        ("path".to_string(), options.path.clone()),
        ("regex".to_string(), options.regex.to_string()),
        (
            "case_sensitive".to_string(),
            options.case_sensitive.to_string(),
        ),
        (
            "context_lines".to_string(),
            options.context_lines.to_string(),
        ),
        ("matches".to_string(), matches.len().to_string()),
        (
            "result_limit_reached".to_string(),
            result_limit_reached.to_string(),
        ),
        ("complete".to_string(), complete.to_string()),
        (
            "scanned_files".to_string(),
            coverage.scanned_files.to_string(),
        ),
        (
            "scanned_bytes".to_string(),
            coverage.scanned_bytes.to_string(),
        ),
        (
            "skipped_hidden_entries".to_string(),
            coverage.skipped_hidden_entries.to_string(),
        ),
        (
            "skipped_symlinks".to_string(),
            coverage.skipped_symlinks.to_string(),
        ),
        (
            "skipped_glob_files".to_string(),
            coverage.skipped_glob_files.to_string(),
        ),
        (
            "unreadable_files".to_string(),
            coverage.unreadable_files.to_string(),
        ),
        (
            "truncated_files".to_string(),
            coverage.truncated_files.to_string(),
        ),
        (
            "discovery_limit_reached".to_string(),
            coverage.discovery_limit_reached.to_string(),
        ),
        (
            "file_limit_reached".to_string(),
            coverage.file_limit_reached.to_string(),
        ),
        (
            "byte_limit_reached".to_string(),
            coverage.byte_limit_reached.to_string(),
        ),
    ]);
    if let Some(glob) = options.glob.as_ref() {
        metadata.insert("glob".to_string(), glob.clone());
    }
    if let Some(cursor) = next_cursor.as_ref() {
        metadata.insert("next_cursor".to_string(), cursor.clone());
    }

    let structured_results = matches
        .iter()
        .map(|item| {
            serde_json::json!({
                "path": item.path,
                "line": item.line,
                "text": item.text,
                "before": context_json(&item.before),
                "after": context_json(&item.after),
            })
        })
        .collect::<Vec<_>>();
    let structured_output = serde_json::json!({
        "schema": "cindx.file-search-result.v2",
        "query": options.query,
        "path": options.path,
        "regex": options.regex,
        "case_sensitive": options.case_sensitive,
        "glob": options.glob,
        "context_lines": options.context_lines,
        "matches": matches.len(),
        "results": structured_results,
        "next_cursor": next_cursor,
        "result_limit_reached": result_limit_reached,
        "complete": complete,
        "scanned_files": coverage.scanned_files,
        "scanned_bytes": coverage.scanned_bytes,
        "skipped_hidden_entries": coverage.skipped_hidden_entries,
        "skipped_symlinks": coverage.skipped_symlinks,
        "skipped_glob_files": coverage.skipped_glob_files,
        "unreadable_files": coverage.unreadable_files,
        "truncated_files": coverage.truncated_files,
        "discovery_limit_reached": coverage.discovery_limit_reached,
        "file_limit_reached": coverage.file_limit_reached,
        "byte_limit_reached": coverage.byte_limit_reached,
    });
    let next_action = if cancelled {
        Some(
            "Retry only if the search is still required, preferably with a narrower path or query."
                .to_string(),
        )
    } else if let Some(cursor) = metadata.get("next_cursor") {
        Some(format!(
            "Continue with file.search using cursor={cursor} and exactly the same options. Narrow the path, glob, or query when possible."
        ))
    } else if !complete || evidence_truncated {
        Some(
            "Narrow the path, glob, or query; this call did not expose every possible match."
                .to_string(),
        )
    } else {
        None
    };
    let summary = if cancelled {
        "File search was cancelled.".to_string()
    } else if matches.is_empty() {
        "No matches were found in the searched scope.".to_string()
    } else {
        format!("Found {} matches in the searched scope.", matches.len())
    };
    let observation = ToolObservationV2::new(
        "file.search",
        summary,
        evidence,
        complete && !evidence_truncated,
        metadata.clone(),
    );
    let mut result = tool_result(
        invocation_id,
        if cancelled {
            ToolOutcomeStatus::Cancelled
        } else {
            ToolOutcomeStatus::Succeeded
        },
        output,
        metadata,
    );
    result.structured_output_json = Some(structured_output.to_string());
    result.model_observation = Some(match next_action {
        Some(next_action) => observation.with_next_action(next_action),
        None => observation,
    });
    result
}

fn render_matches(matches: &[SearchMatch], context_lines: usize) -> String {
    if context_lines == 0 {
        return matches
            .iter()
            .map(|item| format!("{}:{}:{}", item.path, item.line, item.text))
            .collect::<Vec<_>>()
            .join("\n");
    }
    matches
        .iter()
        .map(|item| {
            item.before
                .iter()
                .map(|line| format!("{}-{}-{}", item.path, line.line, line.text))
                .chain(std::iter::once(format!(
                    "{}:{}:{}",
                    item.path, item.line, item.text
                )))
                .chain(
                    item.after
                        .iter()
                        .map(|line| format!("{}-{}-{}", item.path, line.line, line.text)),
                )
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n--\n")
}

fn context_json(lines: &[SearchContextLine]) -> Vec<serde_json::Value> {
    lines
        .iter()
        .map(|line| serde_json::json!({ "line": line.line, "text": line.text }))
        .collect()
}

pub(crate) fn encode_search_cursor(
    option_hash: u64,
    snapshot_hash: u64,
    next_path: &str,
    next_line: usize,
    next_byte: usize,
) -> String {
    let payload = serde_json::json!({
        "schema": "cindx.file-search-cursor.v2",
        "option_hash": format!("{option_hash:016x}"),
        "snapshot_hash": format!("{snapshot_hash:016x}"),
        "next_path": next_path,
        "next_line": next_line,
        "next_byte": next_byte,
    })
    .to_string();
    format!("{SEARCH_CURSOR_PREFIX}{}", hex_encode(payload.as_bytes()))
}

pub(crate) fn decode_search_cursor(
    encoded: &str,
    expected_option_hash: u64,
    expected_snapshot_hash: u64,
) -> Result<SearchCursor, ToolError> {
    if encoded.len() > SEARCH_CURSOR_MAX_BYTES || !encoded.starts_with(SEARCH_CURSOR_PREFIX) {
        return Err(invalid_search_cursor("cursor envelope is invalid"));
    }
    let bytes = hex_decode(&encoded[SEARCH_CURSOR_PREFIX.len()..])?;
    let payload: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| invalid_search_cursor("cursor payload is invalid"))?;
    let expected_option_hash = format!("{expected_option_hash:016x}");
    let expected_snapshot_hash = format!("{expected_snapshot_hash:016x}");
    if payload.get("schema").and_then(serde_json::Value::as_str)
        != Some("cindx.file-search-cursor.v2")
        || payload
            .get("option_hash")
            .and_then(serde_json::Value::as_str)
            != Some(expected_option_hash.as_str())
        || payload
            .get("snapshot_hash")
            .and_then(serde_json::Value::as_str)
            != Some(expected_snapshot_hash.as_str())
    {
        return Err(invalid_search_cursor(
            "cursor does not match the workspace snapshot and search options",
        ));
    }
    let next_path = payload
        .get("next_path")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_search_cursor("cursor path is missing"))?
        .to_string();
    let next_line = payload
        .get("next_line")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_search_cursor("cursor line is invalid"))?;
    let next_byte = payload
        .get("next_byte")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| invalid_search_cursor("cursor byte offset is invalid"))?;
    Ok(SearchCursor {
        next_path,
        next_line,
        next_byte,
    })
}

pub(crate) fn invalid_search_cursor(message: &str) -> ToolError {
    ToolError {
        code: "invalid_cursor".to_string(),
        message: message.to_string(),
        retryable: false,
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(value: &str) -> Result<Vec<u8>, ToolError> {
    if value.is_empty() || !value.len().is_multiple_of(2) {
        return Err(invalid_search_cursor("cursor encoding is invalid"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])
                .ok_or_else(|| invalid_search_cursor("cursor encoding is invalid"))?;
            let low = hex_digit(pair[1])
                .ok_or_else(|| invalid_search_cursor("cursor encoding is invalid"))?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
