use super::test_support::temp_test_root;
use super::*;

#[test]
fn redacts_sensitive_values_in_new_and_existing_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::Tool,
        "api_key=secret-value\nBearer token-value\nsk-1234567890abcdef",
    )
    .expect("message should append");
    store
        .append(Event {
            id: EventId("legacy-secret".to_string()),
            task_id: phase4_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::ToolCallFinished,
            summary: "legacy secret".to_string(),
            metadata: [("output".to_string(), "password=old-secret".to_string())]
                .into_iter()
                .collect(),
        })
        .expect("legacy event should append");

    let updated = redact_persisted_events(&mut store).expect("history should redact");
    let events = store
        .list_by_task(&phase4_task_id())
        .expect("events should load");
    let rendered = events
        .iter()
        .flat_map(|event| event.metadata.values())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(updated, 1);
    assert!(!rendered.contains("secret-value"));
    assert!(!rendered.contains("token-value"));
    assert!(!rendered.contains("1234567890abcdef"));
    assert!(!rendered.contains("old-secret"));
    assert!(rendered.contains("[REDACTED]"));
}

#[test]
fn redacting_tool_call_metadata_preserves_nested_json() {
    let arguments = serde_json::json!({
        "command": "cat provider.conf | sed -E 's/(key|token|secret|api[_-]?key)=.*/[REDACTED]/g'",
        "api_key": "secret-value"
    })
    .to_string();
    let raw_tool_calls = serde_json::json!([{
        "id": "call_1",
        "type": "function",
        "function": {
            "name": "shell_run",
            "arguments": arguments
        }
    }])
    .to_string();
    let metadata = [("raw_tool_calls_json".to_string(), raw_tool_calls)]
        .into_iter()
        .collect();

    let redacted = redact_metadata(&metadata);
    let parsed: serde_json::Value = serde_json::from_str(
        redacted
            .get("raw_tool_calls_json")
            .expect("tool calls should remain present"),
    )
    .expect("tool calls should remain valid JSON");
    let nested: serde_json::Value = serde_json::from_str(
        parsed[0]["function"]["arguments"]
            .as_str()
            .expect("arguments should remain a JSON string"),
    )
    .expect("tool arguments should remain valid JSON");

    assert_eq!(nested["api_key"], "[REDACTED]");
    assert!(nested["command"].as_str().is_some());
    assert!(!redacted["raw_tool_calls_json"].contains("secret-value"));
}

#[test]
fn generated_project_ids_are_not_mistaken_for_prefixed_secrets() {
    for name in ["SK Model", "AKIA Research"] {
        let project_id = new_project_id(name);
        let metadata = [
            ("project_id".to_string(), project_id.clone()),
            (
                "content".to_string(),
                "token=sk-abcdefghijklmnop".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let redacted = redact_metadata(&metadata);
        assert_eq!(redacted.get("project_id"), Some(&project_id));
        assert_eq!(
            redacted.get("content").map(String::as_str),
            Some("token=[REDACTED]")
        );
    }
}

#[test]
fn event_redaction_marker_records_completed_migration() {
    let root = temp_test_root("cindx-event-redaction-marker");
    assert!(!event_redaction_complete(&root));

    mark_event_redaction_complete(&root).expect("marker should persist");

    assert!(event_redaction_complete(&root));
    assert_eq!(
        fs::read_to_string(event_redaction_marker_path(&root)).expect("marker should be readable"),
        "events-redaction-v1\n"
    );
    fs::remove_dir_all(root).expect("marker fixture should be removed");
}
