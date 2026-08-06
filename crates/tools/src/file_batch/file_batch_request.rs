use std::collections::BTreeSet;

use crate::ToolError;

pub(super) const MAX_BATCH_PATHS: usize = 8;
pub(super) const DEFAULT_BYTES_PER_FILE: usize = 32 * 1024;
pub(super) const MAX_BYTES_PER_FILE: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadPath {
    pub(super) path: String,
    pub(super) offset_bytes: u64,
}

pub(super) struct ReadBatchRequest {
    pub(super) paths: Vec<ReadPath>,
    pub(super) max_bytes_per_file: usize,
}

pub(super) fn parse_request(input_json: &str) -> Result<ReadBatchRequest, ToolError> {
    let input: serde_json::Value = serde_json::from_str(input_json)
        .map_err(|error| ToolError::new(format!("invalid file.read_many input: {error}")))?;
    let paths = parse_paths(&input)?;
    let max_bytes_per_file = input
        .get("max_bytes_per_file")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_BYTES_PER_FILE);
    if !(1..=MAX_BYTES_PER_FILE).contains(&max_bytes_per_file) {
        return Err(ToolError::new(format!(
            "max_bytes_per_file must be between 1 and {MAX_BYTES_PER_FILE}"
        )));
    }
    Ok(ReadBatchRequest {
        paths,
        max_bytes_per_file,
    })
}

fn parse_paths(input: &serde_json::Value) -> Result<Vec<ReadPath>, ToolError> {
    let values = input
        .get("paths")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ToolError::new("file.read_many requires a paths array"))?;
    if values.is_empty() || values.len() > MAX_BATCH_PATHS {
        return Err(ToolError::new(format!(
            "paths must contain between 1 and {MAX_BATCH_PATHS} entries"
        )));
    }

    let mut seen = BTreeSet::new();
    let mut paths = Vec::with_capacity(values.len());
    for value in values {
        let request = match value {
            serde_json::Value::String(path) => ReadPath {
                path: non_empty_path(path)?,
                offset_bytes: 0,
            },
            serde_json::Value::Object(fields) => {
                if fields
                    .keys()
                    .any(|key| key != "path" && key != "offset_bytes")
                {
                    return Err(ToolError::new(
                        "file.read_many path objects accept only path and offset_bytes",
                    ));
                }
                let path = fields
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| ToolError::new("every path object requires a path string"))?;
                let offset_bytes = fields
                    .get("offset_bytes")
                    .map(|value| {
                        value.as_u64().ok_or_else(|| {
                            ToolError::new("offset_bytes must be a non-negative integer")
                        })
                    })
                    .transpose()?
                    .unwrap_or(0);
                ReadPath {
                    path: non_empty_path(path)?,
                    offset_bytes,
                }
            }
            _ => {
                return Err(ToolError::new(
                    "every paths entry must be a string or path object",
                ));
            }
        };
        if seen.insert((request.path.clone(), request.offset_bytes)) {
            paths.push(request);
        }
    }
    Ok(paths)
}

fn non_empty_path(path: &str) -> Result<String, ToolError> {
    let path = path.trim();
    if path.is_empty() {
        Err(ToolError::new(
            "every paths entry must contain a non-empty path",
        ))
    } else {
        Ok(path.to_string())
    }
}
