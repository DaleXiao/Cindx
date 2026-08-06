use std::fs;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::{Metadata, TaskId, ToolCallId, ToolInvocation, ToolOutcomeStatus};

use super::file_list_collection::{collect_directory_entries, glob_matches, inspect_entry};
use super::file_list_contract::{DEFAULT_LIST_RESULTS, MAX_LIST_DISCOVERY_ENTRIES};
use super::file_list_cursor::cursor_scope;
use super::file_list_result::build_result;
use super::ListDirectoryTool;
use crate::{Tool, ToolExecutionControl};

fn invocation(input: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        id: ToolCallId("list".to_string()),
        task_id: TaskId("task".to_string()),
        tool_name: "file.list".to_string(),
        input_json: input.to_string(),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    }
}

#[test]
fn default_listing_keeps_raw_rows_and_sorts_by_name() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("z.txt"), "z").expect("write z");
    fs::write(root.path().join("a.txt"), "alpha").expect("write a");

    let result = ListDirectoryTool::new(root.path())
        .execute(invocation(serde_json::json!({ "path": "." })))
        .expect("list");

    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(result.output, "file\t5\ta.txt\nfile\t1\tz.txt");
    assert_eq!(result.metadata.get("entries"), Some(&"2".to_string()));
}

#[test]
fn glob_pages_are_stable_and_cursor_bound_to_scope() {
    let root = tempfile::tempdir().expect("workspace");
    for name in ["b.rs", "a.rs", "notes.md"] {
        fs::write(root.path().join(name), name).expect("write fixture");
    }
    let tool = ListDirectoryTool::new(root.path());
    let first = tool
        .execute(invocation(serde_json::json!({
            "path": ".",
            "glob": "*.rs",
            "max_results": 1
        })))
        .expect("first page");
    let first_structured: serde_json::Value =
        serde_json::from_str(first.structured_output_json.as_deref().unwrap()).unwrap();
    let cursor = first_structured["next_cursor"].as_str().unwrap();
    let second = tool
        .execute(invocation(serde_json::json!({
            "path": ".",
            "glob": "*.rs",
            "max_results": 1,
            "cursor": cursor
        })))
        .expect("second page");
    let second_structured: serde_json::Value =
        serde_json::from_str(second.structured_output_json.as_deref().unwrap()).unwrap();

    assert!(first.output.ends_with("a.rs"));
    assert!(second.output.ends_with("b.rs"));
    assert_eq!(first_structured["complete"], false);
    assert_eq!(second_structured["complete"], true);
    let error = tool
        .execute(invocation(serde_json::json!({
            "path": ".",
            "glob": "*.md",
            "cursor": cursor
        })))
        .expect_err("cursor scope mismatch");
    assert!(error.message.contains("does not match"));
}

#[test]
fn cursor_rejects_a_changed_directory_snapshot() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("a.rs"), "a").expect("write a");
    fs::write(root.path().join("b.rs"), "b").expect("write b");
    let tool = ListDirectoryTool::new(root.path());
    let first = tool
        .execute(invocation(serde_json::json!({
            "path": ".",
            "glob": "*.rs",
            "max_results": 1
        })))
        .expect("first page");
    let first_structured: serde_json::Value =
        serde_json::from_str(first.structured_output_json.as_deref().unwrap()).unwrap();
    let cursor = first_structured["next_cursor"].as_str().unwrap();
    fs::write(root.path().join("aa.rs"), "new").expect("mutate listing");

    let error = tool
        .execute(invocation(serde_json::json!({
            "path": ".",
            "glob": "*.rs",
            "max_results": 1,
            "cursor": cursor
        })))
        .expect_err("changed snapshot must invalidate cursor");

    assert!(error.message.contains("result set changed"));
}

#[test]
fn metadata_failure_is_explicit_and_not_top_level_success() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("ok.txt"), "ok").expect("write ok");
    fs::write(root.path().join("broken.txt"), "broken").expect("write broken");
    let entries = fs::read_dir(root.path()).expect("read directory");
    let snapshot = collect_directory_entries(
        entries,
        None,
        MAX_LIST_DISCOVERY_ENTRIES,
        &|| false,
        &|entry| {
            if entry.file_name() == "broken.txt" {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
            } else {
                inspect_entry(entry)
            }
        },
    );
    let result = build_result(
        invocation(serde_json::json!({ "path": "." })),
        ".".to_string(),
        None,
        DEFAULT_LIST_RESULTS,
        None,
        cursor_scope(".", None),
        snapshot,
    )
    .expect("partial result");
    let structured: serde_json::Value =
        serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Failed);
    assert_eq!(structured["failures"].as_array().unwrap().len(), 1);
    assert_eq!(structured["complete"], false);
    assert!(result.output.ends_with("ok.txt"));
}

#[test]
fn low_hit_glob_stops_at_discovery_limit_without_inspecting_unmatched_entries() {
    const TEST_DISCOVERY_LIMIT: usize = 8;

    let root = tempfile::tempdir().expect("workspace");
    for index in 0..32 {
        fs::write(root.path().join(format!("entry-{index:02}.txt")), "fixture")
            .expect("write fixture");
    }
    let inspections = AtomicUsize::new(0);
    let entries = fs::read_dir(root.path()).expect("read directory");
    let snapshot = collect_directory_entries(
        entries,
        Some("*.rs"),
        TEST_DISCOVERY_LIMIT,
        &|| false,
        &|entry| {
            inspections.fetch_add(1, Ordering::SeqCst);
            inspect_entry(entry)
        },
    );

    assert_eq!(snapshot.discovered, TEST_DISCOVERY_LIMIT);
    assert_eq!(snapshot.discovery_limit, TEST_DISCOVERY_LIMIT);
    assert!(snapshot.discovery_limit_reached);
    assert!(snapshot.entries.is_empty());
    assert!(snapshot.failures.is_empty());
    assert_eq!(inspections.load(Ordering::SeqCst), 0);

    let result = build_result(
        invocation(serde_json::json!({
            "path": ".",
            "glob": "*.rs"
        })),
        ".".to_string(),
        Some("*.rs".to_string()),
        DEFAULT_LIST_RESULTS,
        None,
        cursor_scope(".", Some("*.rs")),
        snapshot,
    )
    .expect("bounded partial result");
    let structured: serde_json::Value =
        serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Failed);
    assert_eq!(
        result.failure.as_ref().map(|failure| failure.code.as_str()),
        Some("file_list_discovery_limit")
    );
    assert_eq!(structured["discovered"], TEST_DISCOVERY_LIMIT);
    assert_eq!(structured["discovery_limit"], TEST_DISCOVERY_LIMIT);
    assert_eq!(structured["discovery_limit_reached"], true);
    assert_eq!(structured["complete"], false);
    assert!(structured["next_cursor"].is_null());
    assert!(structured["entries"].as_array().unwrap().is_empty());
}

#[test]
fn cancellation_is_structured_and_does_not_claim_completion() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("a.txt"), "a").expect("write a");
    fs::write(root.path().join("b.txt"), "b").expect("write b");
    let checks = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&checks);
    let control = ToolExecutionControl::new(move || observed.fetch_add(1, Ordering::SeqCst) >= 1);

    let result = ListDirectoryTool::new(root.path())
        .execute_with_control(invocation(serde_json::json!({ "path": "." })), &control)
        .expect("cancelled result");
    let structured: serde_json::Value =
        serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Cancelled);
    assert_eq!(structured["cancelled"], true);
    assert_eq!(structured["complete"], false);
}

#[test]
fn wildcard_matching_is_bounded_to_the_entry_name() {
    assert!(glob_matches("*.rs", "main.rs"));
    assert!(glob_matches("file-?.txt", "file-a.txt"));
    assert!(!glob_matches("*.rs", "main.ts"));
    assert!(!glob_matches("src/*.rs", "main.rs"));
}
