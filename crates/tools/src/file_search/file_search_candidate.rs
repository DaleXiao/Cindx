use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use glob::{MatchOptions, Pattern};

use super::file_search_types::CandidateFile;
use crate::file_query_contract_v3::SearchCoverage;
use crate::file_tools::is_sensitive_workspace_path;

pub(super) fn add_candidate(
    workspace_root: &Path,
    path: &Path,
    metadata: fs::Metadata,
    pattern: Option<&Pattern>,
    case_sensitive: bool,
    candidates: &mut Vec<CandidateFile>,
    coverage: &mut SearchCoverage,
) {
    if is_sensitive_workspace_path(workspace_root, path) {
        return;
    }
    let Some(relative) = path
        .strip_prefix(workspace_root)
        .ok()
        .and_then(Path::to_str)
        .map(|value| value.replace(std::path::MAIN_SEPARATOR, "/"))
    else {
        coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
        return;
    };
    let matches = pattern.is_none_or(|pattern| {
        pattern.matches_with(
            &relative,
            MatchOptions {
                case_sensitive,
                require_literal_separator: false,
                require_literal_leading_dot: true,
            },
        )
    });
    if !matches {
        coverage.skipped_glob_files = coverage.skipped_glob_files.saturating_add(1);
        return;
    }
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    candidates.push(CandidateFile {
        path: path.to_path_buf(),
        relative,
        size: metadata.len(),
        modified_nanos,
    });
}
