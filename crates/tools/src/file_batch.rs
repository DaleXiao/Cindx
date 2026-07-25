use std::collections::BTreeSet;
use std::path::PathBuf;

use agent_core::{
    Metadata, PermissionRequest, ToolInvocation, ToolOutcomeStatus, ToolRisk, ToolSpec,
};

use super::{tool_result, ReadFileTool, Tool, ToolError};

const MAX_BATCH_PATHS: usize = 8;
const DEFAULT_BYTES_PER_FILE: usize = 32 * 1024;
const MAX_BYTES_PER_FILE: usize = 32 * 1024;

pub struct ReadFilesTool {
    workspace_root: PathBuf,
}

impl ReadFilesTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for ReadFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::builtin(
            "file.read_many",
            "file",
            "Read several bounded UTF-8 files from the workspace in one evidence call.",
            ToolRisk::ReadOnly,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": MAX_BATCH_PATHS,
                        "description": "Workspace-relative file paths to read"
                    },
                    "max_bytes_per_file": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_BYTES_PER_FILE,
                        "description": "Maximum UTF-8 bytes returned for each file"
                    }
                },
                "required": ["paths"],
                "additionalProperties": false
            })
            .to_string(),
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<agent_core::ToolResult, ToolError> {
        let input: serde_json::Value = serde_json::from_str(&invocation.input_json)
            .map_err(|error| ToolError::new(format!("invalid file.read_many input: {error}")))?;
        let paths = parse_paths(&input)?;
        let max_bytes = input
            .get("max_bytes_per_file")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_BYTES_PER_FILE);
        if !(1..=MAX_BYTES_PER_FILE).contains(&max_bytes) {
            return Err(ToolError::new(format!(
                "max_bytes_per_file must be between 1 and {MAX_BYTES_PER_FILE}"
            )));
        }

        let reader = ReadFileTool::new(self.workspace_root.clone());
        let mut sections = Vec::with_capacity(paths.len());
        let mut read_count = 0usize;
        let mut error_count = 0usize;
        for path in &paths {
            let mut child = invocation.clone();
            child.tool_name = "file.read".to_string();
            child.input_json = serde_json::json!({
                "path": path,
                "max_bytes": max_bytes
            })
            .to_string();
            match reader.execute(child) {
                Ok(result) if matches!(result.status, ToolOutcomeStatus::Succeeded) => {
                    read_count += 1;
                    sections.push(format!("===== {path} =====\n{}", result.output));
                }
                Ok(result) => {
                    error_count += 1;
                    sections.push(format!(
                        "===== {path} =====\n[read failed: {}]",
                        result.output
                    ));
                }
                Err(error) => {
                    error_count += 1;
                    sections.push(format!("===== {path} =====\n[read failed: {error}]"));
                }
            }
        }

        let mut metadata = Metadata::new();
        metadata.insert("paths_requested".to_string(), paths.len().to_string());
        metadata.insert("paths_read".to_string(), read_count.to_string());
        metadata.insert("paths_failed".to_string(), error_count.to_string());
        metadata.insert("max_bytes_per_file".to_string(), max_bytes.to_string());
        let status = if read_count > 0 {
            ToolOutcomeStatus::Succeeded
        } else {
            ToolOutcomeStatus::Failed
        };
        Ok(tool_result(
            invocation.id,
            status,
            sections.join("\n\n"),
            metadata,
        ))
    }
}

fn parse_paths(input: &serde_json::Value) -> Result<Vec<String>, ToolError> {
    let values = input
        .get("paths")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ToolError::new("file.read_many requires a paths array"))?;
    if values.is_empty() || values.len() > MAX_BATCH_PATHS {
        return Err(ToolError::new(format!(
            "paths must contain between 1 and {MAX_BATCH_PATHS} entries"
        )));
    }

    let mut unique = BTreeSet::new();
    for value in values {
        let path = value
            .as_str()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| ToolError::new("every paths entry must be a non-empty string"))?;
        unique.insert(path.to_string());
    }
    Ok(unique.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use agent_core::{TaskId, ToolCallId};

    use super::*;

    fn invocation(input: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId("batch-read".to_string()),
            task_id: TaskId("task".to_string()),
            tool_name: "file.read_many".to_string(),
            input_json: input.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        }
    }

    #[test]
    fn reads_multiple_workspace_files_in_one_call() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cindx-read-many-{unique}"));
        fs::create_dir_all(root.join("release")).expect("create fixture");
        fs::write(
            root.join("release/manifest.toml"),
            "codename = \"Silver Current\"\n",
        )
        .expect("write manifest");
        fs::write(root.join("release/approvals.md"), "Ticket: SAFE-2718\n")
            .expect("write approvals");

        let result = ReadFilesTool::new(&root)
            .execute(invocation(serde_json::json!({
                "paths": ["release/manifest.toml", "release/approvals.md"]
            })))
            .expect("batch read");

        assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
        assert!(result.output.contains("Silver Current"));
        assert!(result.output.contains("SAFE-2718"));
        assert_eq!(result.metadata.get("paths_read"), Some(&"2".to_string()));
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn enforces_batch_size_limit() {
        let paths = (0..=MAX_BATCH_PATHS)
            .map(|index| format!("{index}.txt"))
            .collect::<Vec<_>>();
        let error = ReadFilesTool::new(std::env::temp_dir())
            .execute(invocation(serde_json::json!({ "paths": paths })))
            .expect_err("oversized batch must fail");
        assert!(error.message.contains("between 1 and 8"));
    }
}
