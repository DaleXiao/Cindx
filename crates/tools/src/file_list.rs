mod file_list_collection;
mod file_list_contract;
mod file_list_cursor;
mod file_list_model;
mod file_list_result;

#[cfg(test)]
mod file_list_tests;

use std::{fs, path::PathBuf};

use agent_core::{PermissionRequest, ToolInvocation, ToolResult, ToolSpec};

use self::file_list_collection::{collect_directory_entries, inspect_entry};
use self::file_list_contract::{
    file_list_spec, DEFAULT_LIST_RESULTS, MAX_LIST_DISCOVERY_ENTRIES, MAX_LIST_RESULTS,
};
use self::file_list_cursor::{cursor_scope, decode_cursor};
use self::file_list_result::build_result;
use super::{
    parse_bounded_usize_input, parse_input, resolve_workspace_path, resolve_workspace_read_path,
    Tool, ToolError, ToolExecutionControl,
};

pub struct ListDirectoryTool {
    workspace_root: PathBuf,
}

impl ListDirectoryTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    fn execute_list(
        &self,
        invocation: ToolInvocation,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let path = input
            .get("path")
            .cloned()
            .unwrap_or_else(|| ".".to_string());
        let glob = input
            .get("glob")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let max_results = parse_bounded_usize_input(
            &input,
            "max_results",
            DEFAULT_LIST_RESULTS,
            1,
            MAX_LIST_RESULTS,
        )?;
        let scope = cursor_scope(&path, glob.as_deref());
        let cursor = input
            .get("cursor")
            .map(|cursor| decode_cursor(cursor, scope))
            .transpose()?;
        let resolved = resolve_workspace_path(&self.workspace_root, &path)?;
        let resolved = resolve_workspace_read_path(&self.workspace_root, &resolved)?;
        let entries = fs::read_dir(&resolved)
            .map_err(|error| ToolError::new(format!("failed to list directory: {error}")))?;
        let snapshot = collect_directory_entries(
            entries,
            glob.as_deref(),
            MAX_LIST_DISCOVERY_ENTRIES,
            should_cancel,
            &inspect_entry,
        );
        build_result(invocation, path, glob, max_results, cursor, scope, snapshot)
    }
}

impl Tool for ListDirectoryTool {
    fn spec(&self) -> ToolSpec {
        file_list_spec()
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.execute_list(invocation, &|| false)
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        self.execute_list(invocation, &|| control.should_cancel())
    }
}
