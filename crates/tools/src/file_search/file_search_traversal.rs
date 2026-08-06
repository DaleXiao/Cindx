use std::fs;
use std::path::Path;

use glob::Pattern;

use super::file_search_candidate::add_candidate;
use super::file_search_types::CandidateFile;
use crate::file_query_contract_v3::SearchCoverage;
use crate::ToolError;

const SEARCH_MAX_DISCOVERED_FILES: usize = 16_384;
const SEARCH_MAX_DISCOVERED_PATH_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn discover_candidates(
    workspace_root: &Path,
    root: &Path,
    pattern: Option<&Pattern>,
    case_sensitive: bool,
    coverage: &mut SearchCoverage,
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<CandidateFile>, ToolError> {
    discover_with_budget(
        workspace_root,
        root,
        pattern,
        case_sensitive,
        coverage,
        DiscoveryBudget::default(),
        should_cancel,
    )
}

#[cfg(test)]
pub(super) fn discover_candidates_with_limits(
    workspace_root: &Path,
    root: &Path,
    pattern: Option<&Pattern>,
    case_sensitive: bool,
    coverage: &mut SearchCoverage,
    max_entries: usize,
    max_path_bytes: usize,
) -> Result<Vec<CandidateFile>, ToolError> {
    discover_with_budget(
        workspace_root,
        root,
        pattern,
        case_sensitive,
        coverage,
        DiscoveryBudget::with_limits(max_entries, max_path_bytes),
        &|| false,
    )
}

fn discover_with_budget(
    workspace_root: &Path,
    root: &Path,
    pattern: Option<&Pattern>,
    case_sensitive: bool,
    coverage: &mut SearchCoverage,
    budget: DiscoveryBudget,
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<CandidateFile>, ToolError> {
    let mut candidates = Vec::new();
    let mut discovery = DiscoveryContext {
        workspace_root,
        pattern,
        case_sensitive,
        coverage,
        budget,
        should_cancel,
    };
    discovery.collect(root, true, &mut candidates)?;
    candidates.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(candidates)
}

struct DiscoveryBudget {
    entries: usize,
    path_bytes: usize,
    max_entries: usize,
    max_path_bytes: usize,
}

impl Default for DiscoveryBudget {
    fn default() -> Self {
        Self::with_limits(
            SEARCH_MAX_DISCOVERED_FILES,
            SEARCH_MAX_DISCOVERED_PATH_BYTES,
        )
    }
}

impl DiscoveryBudget {
    fn with_limits(max_entries: usize, max_path_bytes: usize) -> Self {
        Self {
            entries: 0,
            path_bytes: 0,
            max_entries,
            max_path_bytes,
        }
    }

    fn record(&mut self, path_bytes: usize) -> bool {
        if self.entries >= self.max_entries
            || self.path_bytes.saturating_add(path_bytes) > self.max_path_bytes
        {
            return false;
        }
        self.entries += 1;
        self.path_bytes = self.path_bytes.saturating_add(path_bytes);
        true
    }
}

struct DiscoveryContext<'a> {
    workspace_root: &'a Path,
    pattern: Option<&'a Pattern>,
    case_sensitive: bool,
    coverage: &'a mut SearchCoverage,
    budget: DiscoveryBudget,
    should_cancel: &'a dyn Fn() -> bool,
}

impl DiscoveryContext<'_> {
    fn collect(
        &mut self,
        path: &Path,
        root: bool,
        candidates: &mut Vec<CandidateFile>,
    ) -> Result<(), ToolError> {
        if (self.should_cancel)() {
            return Ok(());
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if root => {
                return Err(ToolError::new(format!(
                    "failed to read search path: {error}"
                )))
            }
            Err(_) => {
                self.coverage.unreadable_files = self.coverage.unreadable_files.saturating_add(1);
                return Ok(());
            }
        };
        if root && metadata.is_file() && !self.record_path(path) {
            return Ok(());
        }
        if metadata.file_type().is_symlink() {
            self.coverage.skipped_symlinks = self.coverage.skipped_symlinks.saturating_add(1);
            return Ok(());
        }
        if metadata.is_file() {
            add_candidate(
                self.workspace_root,
                path,
                metadata,
                self.pattern,
                self.case_sensitive,
                candidates,
                self.coverage,
            );
            return Ok(());
        }
        if self.coverage.discovery_limit_reached {
            return Ok(());
        }
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if root => {
                return Err(ToolError::new(format!(
                    "failed to search directory: {error}"
                )))
            }
            Err(_) => {
                self.coverage.unreadable_files = self.coverage.unreadable_files.saturating_add(1);
                return Ok(());
            }
        };
        let mut discovered = Vec::new();
        for entry in entries {
            if (self.should_cancel)() {
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    if !self.budget.record(0) {
                        self.coverage.discovery_limit_reached = true;
                        break;
                    }
                    self.coverage.unreadable_files =
                        self.coverage.unreadable_files.saturating_add(1);
                    continue;
                }
            };
            if !self.record_path(&entry.path()) {
                break;
            }
            discovered.push(entry);
        }
        discovered.sort_by_key(|entry| entry.file_name());
        for entry in discovered {
            if (self.should_cancel)() {
                break;
            }
            if entry.file_name().to_string_lossy().starts_with('.') {
                self.coverage.skipped_hidden_entries =
                    self.coverage.skipped_hidden_entries.saturating_add(1);
                continue;
            }
            self.collect(&entry.path(), false, candidates)?;
        }
        Ok(())
    }

    fn record_path(&mut self, path: &Path) -> bool {
        let Some(relative) = path
            .strip_prefix(self.workspace_root)
            .ok()
            .and_then(Path::to_str)
        else {
            self.coverage.unreadable_files = self.coverage.unreadable_files.saturating_add(1);
            return false;
        };
        if self.budget.record(relative.len()) {
            true
        } else {
            self.coverage.discovery_limit_reached = true;
            false
        }
    }
}
