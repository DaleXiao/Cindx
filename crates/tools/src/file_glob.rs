use std::fs;
use std::path::{Component, Path, PathBuf};

use agent_core::{
    Metadata, PermissionRequest, ToolExecutionConcurrency, ToolInvocation, ToolOutcomeStatus,
    ToolResult, ToolRisk, ToolSpec,
};
use glob::{MatchOptions, Pattern};

use super::file_tools::is_sensitive_workspace_path;
use super::{
    bounded_model_text, model_observation, parse_bounded_usize_input, parse_input, required_input,
    tool_result, Tool, ToolError, ToolExecutionControl,
};

const FILE_GLOB_RESULT_SCHEMA: &str = "cindx.file-glob-result.v1";
const GLOB_MAX_RESULTS: usize = 200;
const GLOB_MAX_DISCOVERED_ENTRIES: usize = 16_384;
const MODEL_GLOB_EVIDENCE_CHARS: usize = 5_200;

/// `*` and `?` never cross a path separator, while a full-component `**`
/// matches zero or more components (glob-crate matching semantics).
const GLOB_MATCH_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

pub struct GlobFilesTool {
    workspace_root: PathBuf,
}

impl GlobFilesTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    fn execute_glob(
        &self,
        invocation: ToolInvocation,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<ToolResult, ToolError> {
        if should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "File glob cancelled before it started.",
                Metadata::new(),
            ));
        }
        let input = parse_input(&invocation.input_json);
        let pattern = required_input(&input, "pattern")?;
        let pattern = pattern.trim();
        validate_glob_pattern(pattern)?;
        let pattern = Pattern::new(pattern).map_err(|error| ToolError {
            code: "invalid_glob".to_string(),
            message: format!("invalid glob pattern: {error}"),
            retryable: false,
        })?;
        let max_results = parse_bounded_usize_input(
            &input,
            "max_results",
            GLOB_MAX_RESULTS,
            1,
            GLOB_MAX_RESULTS,
        )?;
        let workspace = fs::canonicalize(&self.workspace_root)
            .map_err(|error| ToolError::new(format!("failed to resolve workspace: {error}")))?;
        let mut coverage = GlobCoverage::default();
        let mut matches = Vec::new();
        collect_glob_matches(
            &workspace,
            &workspace,
            &pattern,
            &mut matches,
            &mut coverage,
            should_cancel,
        );
        let cancelled = should_cancel();
        matches.sort();
        Ok(build_glob_result(
            invocation.id,
            pattern.as_str(),
            max_results,
            matches,
            coverage,
            cancelled,
        ))
    }
}

impl Tool for GlobFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::builtin(
            "file.glob",
            "file",
            "Match workspace files with a relative glob pattern such as src/**/*.rs, sorted and bounded to 200 results with a truncated flag. Hidden entries, symlinks, and local credential files are skipped.",
            ToolRisk::ReadOnly,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "minLength": 1, "description": "Workspace-relative glob pattern (for example src/**/*.rs)." },
                    "max_results": { "type": "integer", "minimum": 1, "maximum": GLOB_MAX_RESULTS, "default": GLOB_MAX_RESULTS, "description": "Maximum matched paths returned." }
                },
                "required": ["pattern"],
                "additionalProperties": false
            })
            .to_string(),
        )
        .with_output_schema(
            serde_json::json!({
                "type": "object",
                "properties": {
                    "schema": { "const": FILE_GLOB_RESULT_SCHEMA },
                    "pattern": { "type": "string" },
                    "results": { "type": "array", "items": { "type": "string" } },
                    "returned": { "type": "integer", "minimum": 0 },
                    "max_results": { "type": "integer", "minimum": 1 },
                    "truncated": { "type": "boolean" },
                    "complete": { "type": "boolean" },
                    "cancelled": { "type": "boolean" },
                    "scanned_entries": { "type": "integer", "minimum": 0 },
                    "skipped_hidden_entries": { "type": "integer", "minimum": 0 },
                    "skipped_symlinks": { "type": "integer", "minimum": 0 },
                    "skipped_sensitive_files": { "type": "integer", "minimum": 0 },
                    "unreadable_entries": { "type": "integer", "minimum": 0 },
                    "discovery_limit_reached": { "type": "boolean" }
                },
                "required": ["schema", "pattern", "results", "returned", "max_results", "truncated", "complete", "cancelled", "scanned_entries", "skipped_hidden_entries", "skipped_symlinks", "skipped_sensitive_files", "unreadable_entries", "discovery_limit_reached"],
                "additionalProperties": false
            })
            .to_string(),
        )
        .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.execute_glob(invocation, &|| false)
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        self.execute_glob(invocation, &|| control.should_cancel())
    }
}

/// Reject patterns that could name anything outside the workspace, mirroring
/// the `resolve_workspace_path` component boundary. Traversal itself starts at
/// the canonical workspace root and never follows symlinks, so a matching path
/// cannot escape; this validation makes the boundary explicit to the caller.
fn validate_glob_pattern(pattern: &str) -> Result<(), ToolError> {
    let candidate = Path::new(pattern);
    if candidate.is_absolute() {
        return Err(ToolError::new("absolute glob patterns are not allowed"));
    }
    for component in candidate.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return Err(ToolError::new("glob pattern escapes the workspace"));
        }
    }
    Ok(())
}

#[derive(Default)]
struct GlobCoverage {
    scanned_entries: usize,
    skipped_hidden_entries: usize,
    skipped_symlinks: usize,
    skipped_sensitive_files: usize,
    unreadable_entries: usize,
    discovery_limit_reached: bool,
}

fn collect_glob_matches(
    workspace: &Path,
    directory: &Path,
    pattern: &Pattern,
    matches: &mut Vec<String>,
    coverage: &mut GlobCoverage,
    should_cancel: &dyn Fn() -> bool,
) {
    if should_cancel() || coverage.discovery_limit_reached {
        return;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => {
            coverage.unreadable_entries = coverage.unreadable_entries.saturating_add(1);
            return;
        }
    };
    let mut discovered = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => discovered.push(entry),
            Err(_) => {
                coverage.unreadable_entries = coverage.unreadable_entries.saturating_add(1);
            }
        }
    }
    discovered.sort_by_key(|entry| entry.file_name());
    for entry in discovered {
        if should_cancel() {
            return;
        }
        if coverage.scanned_entries >= GLOB_MAX_DISCOVERED_ENTRIES {
            coverage.discovery_limit_reached = true;
            return;
        }
        coverage.scanned_entries = coverage.scanned_entries.saturating_add(1);
        if entry.file_name().to_string_lossy().starts_with('.') {
            coverage.skipped_hidden_entries = coverage.skipped_hidden_entries.saturating_add(1);
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                coverage.unreadable_entries = coverage.unreadable_entries.saturating_add(1);
                continue;
            }
        };
        if file_type.is_symlink() {
            coverage.skipped_symlinks = coverage.skipped_symlinks.saturating_add(1);
            continue;
        }
        if file_type.is_dir() {
            collect_glob_matches(
                workspace,
                &entry.path(),
                pattern,
                matches,
                coverage,
                should_cancel,
            );
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if is_sensitive_workspace_path(workspace, &path) {
            coverage.skipped_sensitive_files = coverage.skipped_sensitive_files.saturating_add(1);
            continue;
        }
        let Some(relative) = path
            .strip_prefix(workspace)
            .ok()
            .and_then(Path::to_str)
            .map(|value| value.replace(std::path::MAIN_SEPARATOR, "/"))
        else {
            coverage.unreadable_entries = coverage.unreadable_entries.saturating_add(1);
            continue;
        };
        if pattern.matches_with(&relative, GLOB_MATCH_OPTIONS) {
            matches.push(relative);
        }
    }
}

fn build_glob_result(
    invocation_id: agent_core::ToolCallId,
    pattern: &str,
    max_results: usize,
    matches: Vec<String>,
    coverage: GlobCoverage,
    cancelled: bool,
) -> ToolResult {
    let truncated = matches.len() > max_results;
    let matches = matches.into_iter().take(max_results).collect::<Vec<_>>();
    let returned = matches.len();
    let complete = !cancelled
        && !truncated
        && !coverage.discovery_limit_reached
        && coverage.unreadable_entries == 0;
    let output = if cancelled {
        "File glob cancelled.".to_string()
    } else {
        matches.join("\n")
    };
    let metadata = Metadata::from([
        ("pattern".to_string(), pattern.to_string()),
        ("returned".to_string(), returned.to_string()),
        ("max_results".to_string(), max_results.to_string()),
        ("truncated".to_string(), truncated.to_string()),
        ("complete".to_string(), complete.to_string()),
        ("cancelled".to_string(), cancelled.to_string()),
        (
            "scanned_entries".to_string(),
            coverage.scanned_entries.to_string(),
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
            "skipped_sensitive_files".to_string(),
            coverage.skipped_sensitive_files.to_string(),
        ),
        (
            "unreadable_entries".to_string(),
            coverage.unreadable_entries.to_string(),
        ),
        (
            "discovery_limit_reached".to_string(),
            coverage.discovery_limit_reached.to_string(),
        ),
    ]);
    let structured_output = serde_json::json!({
        "schema": FILE_GLOB_RESULT_SCHEMA,
        "pattern": pattern,
        "results": matches,
        "returned": returned,
        "max_results": max_results,
        "truncated": truncated,
        "complete": complete,
        "cancelled": cancelled,
        "scanned_entries": coverage.scanned_entries,
        "skipped_hidden_entries": coverage.skipped_hidden_entries,
        "skipped_symlinks": coverage.skipped_symlinks,
        "skipped_sensitive_files": coverage.skipped_sensitive_files,
        "unreadable_entries": coverage.unreadable_entries,
        "discovery_limit_reached": coverage.discovery_limit_reached,
    });
    let (evidence, evidence_truncated) = bounded_model_text(&output, MODEL_GLOB_EVIDENCE_CHARS);
    let summary = if cancelled {
        "File glob was cancelled.".to_string()
    } else if returned == 0 {
        "No files matched the glob pattern.".to_string()
    } else if truncated {
        format!(
            "Matched {returned} files; the result is truncated at the {max_results}-match bound."
        )
    } else {
        format!("Matched {returned} files for the glob pattern.")
    };
    let next_action = if cancelled {
        Some("Retry this glob only if it is still required.".to_string())
    } else if coverage.discovery_limit_reached {
        Some(format!(
            "Directory traversal stopped at the hard {GLOB_MAX_DISCOVERED_ENTRIES}-entry limit; narrow the pattern before retrying."
        ))
    } else if truncated {
        Some(format!(
            "Narrow the glob pattern; the result was truncated at {max_results} matches."
        ))
    } else {
        None
    };
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
    result.model_observation = Some(model_observation(
        "file.glob",
        summary,
        evidence,
        complete && !evidence_truncated,
        [
            ("pattern".to_string(), pattern.to_string()),
            ("returned".to_string(), returned.to_string()),
            ("truncated".to_string(), truncated.to_string()),
            ("complete".to_string(), complete.to_string()),
            ("cancelled".to_string(), cancelled.to_string()),
            (
                "scanned_entries".to_string(),
                coverage.scanned_entries.to_string(),
            ),
            (
                "discovery_limit_reached".to_string(),
                coverage.discovery_limit_reached.to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        next_action,
    ));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{TaskId, ToolCallId};

    fn temp_workspace() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cindx-file-glob-test-{}",
            crate::stable_hash(&format!("{:?}", std::time::SystemTime::now()))
        ));
        fs::create_dir_all(&path).expect("workspace should be created");
        path
    }

    fn invocation(input_json: String) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId("call-1".to_string()),
            task_id: TaskId("task-1".to_string()),
            tool_name: "file.glob".to_string(),
            input_json,
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        }
    }

    fn seed_rust_tree(root: &Path) {
        fs::create_dir_all(root.join("src/nested")).expect("src tree should be created");
        fs::write(root.join("src/lib.rs"), "lib").expect("fixture should be written");
        fs::write(root.join("src/nested/deep.rs"), "deep").expect("fixture should be written");
        fs::write(root.join("src/readme.md"), "doc").expect("fixture should be written");
    }

    #[test]
    fn glob_matches_recursive_and_direct_patterns() {
        let root = temp_workspace();
        seed_rust_tree(&root);
        let tool = GlobFilesTool::new(&root);

        let recursive = tool
            .execute(invocation(
                serde_json::json!({ "pattern": "src/**/*.rs" }).to_string(),
            ))
            .expect("recursive glob should succeed");
        let direct = tool
            .execute(invocation(
                serde_json::json!({ "pattern": "src/*.rs" }).to_string(),
            ))
            .expect("direct glob should succeed");

        assert_eq!(recursive.output, "src/lib.rs\nsrc/nested/deep.rs");
        assert_eq!(direct.output, "src/lib.rs");
        let structured: serde_json::Value = serde_json::from_str(
            recursive
                .structured_output_json
                .as_deref()
                .expect("file.glob should expose structured output"),
        )
        .expect("structured output should parse");
        assert_eq!(structured["schema"], FILE_GLOB_RESULT_SCHEMA);
        assert_eq!(structured["returned"], 2);
        assert_eq!(structured["truncated"], false);
        assert_eq!(structured["complete"], true);
        let observation = recursive
            .model_observation
            .expect("file.glob should expose a typed model observation");
        assert!(observation.evidence_complete);
        assert_eq!(
            observation.facts.get("pattern").map(String::as_str),
            Some("src/**/*.rs")
        );
    }

    #[test]
    fn glob_rejects_patterns_that_escape_the_workspace() {
        let root = temp_workspace();
        let tool = GlobFilesTool::new(&root);

        let parent = tool
            .execute(invocation(
                serde_json::json!({ "pattern": "../outside/**/*.rs" }).to_string(),
            ))
            .expect_err("parent-directory pattern must be rejected");
        let absolute = tool
            .execute(invocation(
                serde_json::json!({ "pattern": "/etc/**/*.conf" }).to_string(),
            ))
            .expect_err("absolute pattern must be rejected");

        assert!(parent.message.contains("escapes"));
        assert!(absolute.message.contains("absolute"));
    }

    #[test]
    fn glob_results_are_bounded_and_marked_truncated() {
        let root = temp_workspace();
        fs::create_dir_all(root.join("many")).expect("fixture directory should be created");
        for index in 0..5 {
            fs::write(root.join(format!("many/file-{index}.txt")), "x")
                .expect("fixture should be written");
        }
        let tool = GlobFilesTool::new(&root);

        let result = tool
            .execute(invocation(
                serde_json::json!({ "pattern": "many/*.txt", "max_results": 3 }).to_string(),
            ))
            .expect("bounded glob should succeed");

        let lines = result.output.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "many/file-0.txt");
        assert_eq!(
            result.metadata.get("truncated").map(String::as_str),
            Some("true")
        );
        let observation = result
            .model_observation
            .expect("file.glob should expose a typed model observation");
        assert!(!observation.evidence_complete);
        assert!(observation
            .next_action
            .as_deref()
            .is_some_and(|action| action.contains("Narrow")));
    }

    #[test]
    fn glob_default_result_bound_is_two_hundred() {
        let root = temp_workspace();
        for index in 0..205 {
            fs::write(root.join(format!("f{index:03}.log")), "x")
                .expect("fixture should be written");
        }
        let tool = GlobFilesTool::new(&root);

        let result = tool
            .execute(invocation(
                serde_json::json!({ "pattern": "*.log" }).to_string(),
            ))
            .expect("default-bound glob should succeed");

        assert_eq!(result.output.lines().count(), GLOB_MAX_RESULTS);
        assert_eq!(
            result.metadata.get("truncated").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            result
                .metadata
                .get("max_results")
                .and_then(|value| value.parse::<usize>().ok()),
            Some(200)
        );
    }

    #[test]
    fn glob_skips_hidden_symlinked_and_sensitive_entries() {
        let root = temp_workspace();
        fs::write(root.join("visible.txt"), "v").expect("fixture should be written");
        fs::write(root.join(".hidden.txt"), "h").expect("fixture should be written");
        fs::write(root.join("credentials.json"), "{}").expect("fixture should be written");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("visible.txt"), root.join("linked.txt"))
                .expect("symlink should be created");
        }
        let tool = GlobFilesTool::new(&root);

        let result = tool
            .execute(invocation(
                serde_json::json!({ "pattern": "**/*" }).to_string(),
            ))
            .expect("glob should succeed");

        assert_eq!(result.output, "visible.txt");
        assert_eq!(
            result
                .metadata
                .get("skipped_hidden_entries")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            result
                .metadata
                .get("skipped_sensitive_files")
                .map(String::as_str),
            Some("1")
        );
        #[cfg(unix)]
        assert_eq!(
            result.metadata.get("skipped_symlinks").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn glob_rejects_an_out_of_range_max_results() {
        let root = temp_workspace();
        let tool = GlobFilesTool::new(&root);

        let error = tool
            .execute(invocation(
                serde_json::json!({ "pattern": "**/*", "max_results": 201 }).to_string(),
            ))
            .expect_err("max_results above the bound must be rejected");

        assert!(error.message.contains("between 1 and 200"));
    }
}
