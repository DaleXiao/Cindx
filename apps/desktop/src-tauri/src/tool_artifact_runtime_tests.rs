use super::*;
use crate::desktop_prelude::agent_output_artifacts_from_events;

#[test]
fn computer_screenshot_becomes_the_primary_visual_artifact() {
    let root = std::env::temp_dir().join(format!(
        "cindx-screenshot-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(root.join(".cindx/computer-actions"))
        .expect("computer action directory should exist");
    fs::write(root.join(".cindx/computer-actions/request.json"), b"{}")
        .expect("request fixture should write");
    fs::write(root.join(".cindx/computer-actions/screen.png"), b"png")
        .expect("screenshot fixture should write");
    let mut result = ToolResult::text(
        agent_core::ToolCallId("computer-test".to_string()),
        ToolOutcomeStatus::Succeeded,
        "captured",
        [
            (
                "artifact_path".to_string(),
                ".cindx/computer-actions/request.json".to_string(),
            ),
            (
                "screenshot_path".to_string(),
                ".cindx/computer-actions/screen.png".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    );

    materialize_tool_result_artifacts(&mut result, &root).expect("artifacts should materialize");

    assert!(result
        .metadata
        .get("artifact_path")
        .is_some_and(|path| path.ends_with("screen.png")));
    assert_eq!(tool_result_image_paths(&result).len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn repeated_image_outputs_keep_distinct_immutable_versions() {
    let root = std::env::temp_dir().join(format!(
        "cindx-versioned-image-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    let relative_path = "generated-images/cat.png";
    let source = root.join(relative_path);
    fs::create_dir_all(source.parent().expect("image parent should exist"))
        .expect("image directory should be created");

    let materialize = |call_id: &str, bytes: &[u8]| {
        fs::write(&source, bytes).expect("image version should write");
        let mut result = ToolResult::text(
            agent_core::ToolCallId(call_id.to_string()),
            ToolOutcomeStatus::Succeeded,
            "generated",
            [("artifact_path".to_string(), relative_path.to_string())]
                .into_iter()
                .collect(),
        );
        result.artifacts.push(ToolArtifact {
            path: relative_path.to_string(),
            mime_type: Some("image/png".to_string()),
            title: Some("Generated image".to_string()),
        });
        materialize_tool_result_artifacts(&mut result, &root)
            .expect("image artifact should materialize");
        result
    };

    let first = materialize("image-call-one", b"version-one");
    let first_snapshot = first.metadata["artifact_path"].clone();
    let second = materialize("image-call-two", b"version-two");
    let second_snapshot = second.metadata["artifact_path"].clone();

    assert_eq!(first.metadata["source_path"], relative_path);
    assert_eq!(second.metadata["source_path"], relative_path);
    assert_ne!(first_snapshot, second_snapshot);
    assert_eq!(
        fs::read(root.join(&first_snapshot)).expect("first snapshot should remain readable"),
        b"version-one"
    );
    assert_eq!(
        fs::read(root.join(&second_snapshot)).expect("second snapshot should remain readable"),
        b"version-two"
    );
    assert_eq!(first.artifacts.len(), 1);
    assert_eq!(second.artifacts.len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_structured_tool_output_is_materialized_without_inline_duplication() {
    let root = std::env::temp_dir().join(format!(
        "cindx-structured-output-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(&root).expect("test workspace should exist");
    let structured = serde_json::json!({ "payload": "x".repeat(300 * 1024) }).to_string();
    let mut result = ToolResult::text(
        agent_core::ToolCallId("call/unsafe".to_string()),
        ToolOutcomeStatus::Succeeded,
        "bounded preview",
        Metadata::new(),
    );
    result.structured_output_json = Some(structured.clone());

    materialize_tool_result_artifacts(&mut result, &root)
        .expect("structured output should materialize");

    let path = result
        .metadata
        .get("structured_output_path")
        .expect("structured output should expose its artifact path");
    assert!(path.ends_with("call_unsafe-structured.json"));
    assert_eq!(
        fs::read_to_string(path).expect("structured artifact should read"),
        structured
    );
    assert!(result
        .structured_output_json
        .as_deref()
        .is_some_and(|value| value.len() < 512));
    assert!(result
        .metadata
        .get("structured_output")
        .is_some_and(|value| value.len() < 512));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn agent_outputs_accumulate_versioned_files_across_session_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (index, run_id) in ["run-one", "run-two"].into_iter().enumerate() {
        let context = [
            ("session_id".to_string(), "session-alpha".to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
        ]
        .into_iter()
        .collect();
        append_tool_finished_event(
            &mut store,
            &phase16_task_id(),
            &format!("call-{index}"),
            "file.write",
            "succeeded",
            "file written",
            [
                ("path".to_string(), "notes/result.md".to_string()),
                (
                    "source_path".to_string(),
                    "/workspace/notes/result.md".to_string(),
                ),
                (
                    "artifact_path".to_string(),
                    format!("/workspace/.cindx/output-history/{run_id}/result.md"),
                ),
            ]
            .into_iter()
            .collect(),
            Some(&context),
        )
        .expect("tool output should append");
    }
    let context = [
        ("session_id".to_string(), "session-alpha".to_string()),
        ("agent_run_id".to_string(), "run-two".to_string()),
    ]
    .into_iter()
    .collect();
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-list",
        "file.list",
        "succeeded",
        "notes",
        [("path".to_string(), "notes".to_string())]
            .into_iter()
            .collect(),
        Some(&context),
    )
    .expect("read-only tool output should append");

    let events = agent_events_for_session(&store, &phase16_task_id(), Some("session-alpha"))
        .expect("session events should load");
    let outputs = agent_output_artifacts_from_events(&events);
    let manifest = artifact_manifest_message(&events).expect("manifest should exist");

    assert_eq!(outputs.len(), 2);
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.version)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2])
    );
    assert!(outputs
        .iter()
        .all(|output| { output.source_path.as_deref() == Some("/workspace/notes/result.md") }));
    assert!(outputs
        .iter()
        .any(|output| output.run_id.as_deref() == Some("run-one")));
    assert!(outputs
        .iter()
        .any(|output| output.run_id.as_deref() == Some("run-two")));
    assert_eq!(
        manifest.metadata.get("kind").map(String::as_str),
        Some("artifact_manifest")
    );
    assert!(manifest.content.contains("/workspace/notes/result.md"));
    assert!(manifest.content.contains("run-one"));
    assert!(manifest.content.contains("run-two"));
    assert!(manifest.content.contains("latest_version\": 2"));
}
