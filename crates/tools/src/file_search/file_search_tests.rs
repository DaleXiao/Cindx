use std::fs;

use agent_core::{Metadata, TaskId, ToolCallId, ToolInvocation};
use glob::Pattern;

use super::file_search_traversal::discover_candidates_with_limits;
use super::file_search_types::SEARCH_FILE_SCAN_MAX_BYTES;
use super::SearchFilesTool;
use crate::file_query_contract_v3::SearchCoverage;
use crate::Tool;

fn invocation(input: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        id: ToolCallId("search".to_string()),
        task_id: TaskId("task".to_string()),
        tool_name: "file.search".to_string(),
        input_json: input.to_string(),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    }
}

#[test]
fn defaults_preserve_literal_case_sensitive_raw_output() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("b.txt"), "Needle\nneedle two\n").unwrap();
    fs::write(root.path().join("a.txt"), "needle one\n").unwrap();

    let result = SearchFilesTool::new(root.path())
        .execute(invocation(serde_json::json!({ "query": "needle" })))
        .unwrap();

    assert_eq!(result.output, "a.txt:1:needle one\nb.txt:2:needle two");
    let structured: serde_json::Value =
        serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap();
    assert_eq!(structured["schema"], "cindx.file-search-result.v2");
}

#[test]
fn supports_regex_case_glob_and_context() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "before\nTarget 42\nafter\n").unwrap();
    fs::write(root.path().join("src/lib.txt"), "Target 99\n").unwrap();

    let result = SearchFilesTool::new(root.path())
        .execute(invocation(serde_json::json!({
            "query": "target\\s+\\d+",
            "regex": true,
            "case_sensitive": false,
            "glob": "**/*.rs",
            "context_lines": 1
        })))
        .unwrap();

    assert_eq!(
        result.output,
        "src/lib.rs-1-before\nsrc/lib.rs:2:Target 42\nsrc/lib.rs-3-after"
    );
}

#[test]
fn cursor_pages_without_duplicate_or_omitted_matches() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "hit one\nhit two\n").unwrap();
    fs::write(root.path().join("b.txt"), "hit three\n").unwrap();
    let tool = SearchFilesTool::new(root.path());
    let mut cursor = None;
    let mut outputs = Vec::new();
    let mut pages = 0;

    loop {
        pages += 1;
        let mut input = serde_json::json!({ "query": "hit", "max_results": 1 });
        if let Some(value) = cursor.take() {
            input["cursor"] = serde_json::Value::String(value);
        }
        let result = tool.execute(invocation(input)).unwrap();
        if !result.output.is_empty() {
            outputs.push(result.output);
        }
        cursor = result.metadata.get("next_cursor").cloned();
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(
        outputs,
        ["a.txt:1:hit one", "a.txt:2:hit two", "b.txt:1:hit three"]
    );
    assert_eq!(pages, 3, "the final match must not advertise an empty page");
}

#[test]
fn cursor_resumes_from_a_byte_offset_in_dense_files() {
    let root = tempfile::tempdir().unwrap();
    let contents = (0..100)
        .map(|index| format!("hit {index:03}\n"))
        .collect::<String>();
    fs::write(root.path().join("dense.txt"), contents).unwrap();
    let tool = SearchFilesTool::new(root.path());

    let first = tool
        .execute(invocation(
            serde_json::json!({ "query": "hit", "max_results": 2 }),
        ))
        .unwrap();
    let second = tool
        .execute(invocation(serde_json::json!({
            "query": "hit",
            "max_results": 2,
            "cursor": first.metadata.get("next_cursor").unwrap()
        })))
        .unwrap();

    assert_eq!(first.output, "dense.txt:1:hit 000\ndense.txt:2:hit 001");
    assert_eq!(second.output, "dense.txt:3:hit 002\ndense.txt:4:hit 003");
    let first_bytes = first.metadata["scanned_bytes"].parse::<u64>().unwrap();
    let second_bytes = second.metadata["scanned_bytes"].parse::<u64>().unwrap();
    assert!(second_bytes < first_bytes);
}

#[test]
fn discovery_budget_counts_entries_rejected_by_glob() {
    let root = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(root.path().join(name), "hit\n").unwrap();
    }
    let workspace = fs::canonicalize(root.path()).unwrap();
    let pattern = Pattern::new("**/*.rs").unwrap();
    let mut coverage = SearchCoverage::default();
    let candidates = discover_candidates_with_limits(
        &workspace,
        &workspace,
        Some(&pattern),
        true,
        &mut coverage,
        2,
        usize::MAX,
    )
    .unwrap();

    assert!(coverage.discovery_limit_reached);
    assert_eq!(coverage.skipped_glob_files, 2);
    assert!(candidates.is_empty());
}

#[test]
fn truncated_utf8_tail_preserves_the_valid_searchable_prefix() {
    let root = tempfile::tempdir().unwrap();
    let suffix = b"\nneedle\n";
    let mut contents = vec![b'a'; SEARCH_FILE_SCAN_MAX_BYTES as usize - 1 - suffix.len()];
    contents.extend_from_slice(suffix);
    contents.extend_from_slice("€tail".as_bytes());
    fs::write(root.path().join("large.txt"), contents).unwrap();

    let result = SearchFilesTool::new(root.path())
        .execute(invocation(serde_json::json!({ "query": "needle" })))
        .unwrap();

    assert_eq!(result.output, "large.txt:2:needle");
    assert_eq!(result.metadata["truncated_files"], "1");
    assert_eq!(result.metadata["unreadable_files"], "0");
    assert_eq!(result.metadata["complete"], "false");
}

#[test]
fn cursor_is_bound_to_every_search_option() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "hit one\nhit two\n").unwrap();
    let tool = SearchFilesTool::new(root.path());
    let first = tool
        .execute(invocation(
            serde_json::json!({ "query": "hit", "max_results": 1 }),
        ))
        .unwrap();
    let cursor = first.metadata.get("next_cursor").unwrap();

    let error = tool
        .execute(invocation(serde_json::json!({
            "query": "hit",
            "max_results": 1,
            "case_sensitive": false,
            "cursor": cursor
        })))
        .unwrap_err();

    assert_eq!(error.code, "invalid_cursor");
}

#[test]
fn invalid_regex_and_cursor_are_typed_errors() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "text\n").unwrap();
    let tool = SearchFilesTool::new(root.path());
    let regex_error = tool
        .execute(invocation(
            serde_json::json!({ "query": "[", "regex": true }),
        ))
        .unwrap_err();
    assert_eq!(regex_error.code, "invalid_regex");

    let cursor_error = tool
        .execute(invocation(serde_json::json!({
            "query": "text",
            "cursor": "not-a-cursor"
        })))
        .unwrap_err();
    assert_eq!(cursor_error.code, "invalid_cursor");
}
