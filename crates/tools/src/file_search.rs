mod file_search_candidate;
mod file_search_cursor;
mod file_search_input;
mod file_search_scan;
mod file_search_traversal;
mod file_search_types;

#[cfg(test)]
mod file_search_tests;

use std::fs;
use std::path::PathBuf;

use agent_core::{
    Metadata, PermissionRequest, ToolExecutionConcurrency, ToolInvocation, ToolOutcomeStatus,
    ToolResult, ToolSpec,
};

use self::file_search_cursor::{candidate_snapshot_hash, cursor_start, option_binding_hash};
use self::file_search_input::parse_search_input;
use self::file_search_scan::{
    planned_read_bytes, read_candidate, search_contents, SearchScanRequest,
};
use self::file_search_traversal::discover_candidates;
use self::file_search_types::SearchPosition;
use super::file_query_contract_v3::{
    decode_search_cursor, encode_search_cursor, file_search_result, file_search_spec,
    SearchCoverage,
};
use super::{
    file_tools::reject_sensitive_read_path, resolve_workspace_path, resolve_workspace_read_path,
    Tool, ToolError, ToolExecutionControl,
};

const SEARCH_TOTAL_SCAN_MAX_BYTES: u64 = 64 * 1024 * 1024;
const SEARCH_MAX_SCANNED_FILES: usize = 1_024;

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
        let parsed = parse_search_input(&invocation.input_json)?;
        let options = parsed.options;
        let root = resolve_workspace_path(&self.workspace_root, &options.path)?;
        let root = resolve_workspace_read_path(&self.workspace_root, &root)?;
        reject_sensitive_read_path(&self.workspace_root, &root)?;
        let workspace = fs::canonicalize(&self.workspace_root)
            .map_err(|error| ToolError::new(format!("failed to resolve workspace: {error}")))?;
        let mut coverage = SearchCoverage::default();
        let candidates = discover_candidates(
            &workspace,
            &root,
            parsed.path_pattern.as_ref(),
            options.case_sensitive,
            &mut coverage,
            should_cancel,
        )?;
        let snapshot_hash = candidate_snapshot_hash(&candidates, &coverage);
        let option_hash = option_binding_hash(&workspace, &options);
        let cursor = parsed
            .cursor
            .as_deref()
            .map(|encoded| decode_search_cursor(encoded, option_hash, snapshot_hash))
            .transpose()?;
        let (mut candidate_index, mut position) = cursor_start(&candidates, cursor.as_ref())?;
        let mut matches = Vec::new();
        let mut next_position = None;
        let lookahead_limit = options.max_results.saturating_add(1);

        while candidate_index < candidates.len() && !should_cancel() {
            let candidate = &candidates[candidate_index];
            if coverage.scanned_files >= SEARCH_MAX_SCANNED_FILES {
                coverage.file_limit_reached = true;
                next_position = Some((candidate.relative.clone(), position));
                break;
            }
            let planned_bytes = planned_read_bytes(candidate, position, options.context_lines);
            if coverage.scanned_bytes.saturating_add(planned_bytes) > SEARCH_TOTAL_SCAN_MAX_BYTES {
                coverage.byte_limit_reached = true;
                next_position = Some((candidate.relative.clone(), position));
                break;
            }
            let Some(contents) =
                read_candidate(candidate, position, options.context_lines, &mut coverage)?
            else {
                candidate_index += 1;
                position = SearchPosition::default();
                continue;
            };
            let remaining_results = lookahead_limit.saturating_sub(matches.len());
            let cancelled_during_scan = search_contents(
                candidate,
                &contents,
                &parsed.matcher,
                &mut matches,
                SearchScanRequest {
                    position,
                    context_lines: options.context_lines,
                    remaining_results,
                    should_cancel,
                },
            );
            if cancelled_during_scan {
                break;
            }
            if matches.len() >= lookahead_limit {
                break;
            }
            candidate_index += 1;
            position = SearchPosition::default();
        }
        let cancelled = should_cancel();
        let lookahead = if matches.len() > options.max_results {
            matches.pop()
        } else {
            None
        };
        let next_cursor = if cancelled {
            None
        } else if let Some(next_match) = lookahead {
            Some(encode_search_cursor(
                option_hash,
                snapshot_hash,
                &next_match.path,
                next_match.line,
                next_match.byte_offset,
            ))
        } else {
            next_position.map(|(path, position)| {
                encode_search_cursor(
                    option_hash,
                    snapshot_hash,
                    &path,
                    position.line,
                    position.byte,
                )
            })
        };
        Ok(file_search_result(
            invocation.id,
            options,
            matches,
            coverage,
            next_cursor,
            cancelled,
        ))
    }
}

impl Tool for SearchFilesTool {
    fn spec(&self) -> ToolSpec {
        file_search_spec().with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
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
