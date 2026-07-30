use super::{
    builtin_tool_spec,
    file_tools::{is_sensitive_workspace_path, reject_sensitive_read_path},
    parse_input, required_input, resolve_workspace_path, resolve_workspace_read_path, tool_result,
    Tool, ToolError, ToolExecutionControl,
};
use agent_core::{
    Metadata, PermissionRequest, ToolExecutionConcurrency, ToolInvocation, ToolOutcomeStatus,
    ToolResult, ToolRisk, ToolSpec,
};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

const SEARCH_FILE_SCAN_MAX_BYTES: u64 = 8 * 1024 * 1024;
const SEARCH_MATCH_PREVIEW_CHARS: usize = 512;
const SEARCH_CANCEL_POLL_LINES: usize = 128;
const SEARCH_CANCEL_POLL_DIRECTORY_ENTRIES: usize = 32;
const SEARCH_CANCEL_POLL_BYTES: usize = 64 * 1024;

pub struct SearchFilesTool {
    workspace_root: PathBuf,
}

impl SearchFilesTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    fn execute_search(
        &self,
        invocation: ToolInvocation,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<ToolResult, ToolError> {
        if should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "File search cancelled before it started.",
                Metadata::new(),
            ));
        }
        let input = parse_input(&invocation.input_json);
        let query = required_input(&input, "query")?;
        let path = input
            .get("path")
            .cloned()
            .unwrap_or_else(|| ".".to_string());
        let max_results = input
            .get("max_results")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(50)
            .min(200);
        let root = resolve_workspace_path(&self.workspace_root, &path)?;
        let root = resolve_workspace_read_path(&self.workspace_root, &root)?;
        reject_sensitive_read_path(&self.workspace_root, &root)?;
        let mut results = Vec::new();
        search_directory(
            &self.workspace_root,
            &root,
            &query,
            max_results,
            &mut results,
            should_cancel,
        )?;

        let mut metadata = Metadata::new();
        metadata.insert("query".to_string(), query);
        metadata.insert("path".to_string(), path);
        metadata.insert("matches".to_string(), results.len().to_string());
        let cancelled = should_cancel();
        Ok(tool_result(
            invocation.id,
            if cancelled {
                ToolOutcomeStatus::Cancelled
            } else {
                ToolOutcomeStatus::Succeeded
            },
            if cancelled {
                "File search cancelled.".to_string()
            } else {
                results.join("\n")
            },
            metadata,
        ))
    }
}

impl Tool for SearchFilesTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "file.search",
            "Search UTF-8 files inside the workspace for a literal query.",
            ToolRisk::ReadOnly,
            "query=<literal text>\npath=<optional workspace-relative path>\nmax_results=<optional number>",
        )
        .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.execute_search(invocation, &|| false)
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        self.execute_search(invocation, &|| control.should_cancel())
    }
}

fn search_directory(
    workspace_root: &Path,
    directory: &Path,
    query: &str,
    max_results: usize,
    results: &mut Vec<String>,
    should_cancel: &dyn Fn() -> bool,
) -> Result<(), ToolError> {
    if results.len() >= max_results || should_cancel() {
        return Ok(());
    }

    let metadata = fs::metadata(directory)
        .map_err(|error| ToolError::new(format!("failed to read search path: {error}")))?;
    if metadata.is_file() {
        search_file(
            workspace_root,
            directory,
            query,
            max_results,
            results,
            should_cancel,
        )?;
        return Ok(());
    }

    let entries = fs::read_dir(directory)
        .map_err(|error| ToolError::new(format!("failed to search directory: {error}")))?;
    for (entry_index, entry) in entries.enumerate() {
        if results.len() >= max_results
            || (entry_index.is_multiple_of(SEARCH_CANCEL_POLL_DIRECTORY_ENTRIES) && should_cancel())
        {
            break;
        }
        let entry = entry
            .map_err(|error| ToolError::new(format!("failed to read directory entry: {error}")))?;
        let path = entry.path();
        let file_name = entry.file_name();
        if file_name.to_string_lossy().starts_with('.') {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| ToolError::new(format!("failed to read file type: {error}")))?;
        if file_type.is_symlink() {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| ToolError::new(format!("failed to read metadata: {error}")))?;
        if metadata.is_dir() {
            search_directory(
                workspace_root,
                &path,
                query,
                max_results,
                results,
                should_cancel,
            )?;
        } else if metadata.is_file() {
            search_file(
                workspace_root,
                &path,
                query,
                max_results,
                results,
                should_cancel,
            )?;
        }
    }

    Ok(())
}

fn search_file(
    workspace_root: &Path,
    path: &Path,
    query: &str,
    max_results: usize,
    results: &mut Vec<String>,
    should_cancel: &dyn Fn() -> bool,
) -> Result<(), ToolError> {
    if results.len() >= max_results || should_cancel() {
        return Ok(());
    }
    if is_sensitive_workspace_path(workspace_root, path) {
        return Ok(());
    }
    let Ok(file) = fs::File::open(path) else {
        return Ok(());
    };
    let relative = path.strip_prefix(workspace_root).unwrap_or(path);
    let mut reader = BufReader::new(file.take(SEARCH_FILE_SCAN_MAX_BYTES));
    let mut line = String::new();
    let mut index = 0usize;
    let mut bytes_since_cancel_poll = 0usize;
    loop {
        let poll_cancellation = index.is_multiple_of(SEARCH_CANCEL_POLL_LINES)
            || bytes_since_cancel_poll >= SEARCH_CANCEL_POLL_BYTES;
        if results.len() >= max_results || (poll_cancellation && should_cancel()) {
            break;
        }
        if poll_cancellation {
            bytes_since_cancel_poll = 0;
        }
        line.clear();
        let Ok(read) = reader.read_line(&mut line) else {
            break;
        };
        if read == 0 {
            break;
        }
        bytes_since_cancel_poll = bytes_since_cancel_poll.saturating_add(read);
        index += 1;
        if line.contains(query) {
            let preview: String = line
                .trim()
                .chars()
                .take(SEARCH_MATCH_PREVIEW_CHARS)
                .collect();
            results.push(format!("{}:{}:{}", relative.display(), index, preview));
        }
    }
    Ok(())
}
