use super::*;

#[test]
fn tool_event_metadata_is_compacted_before_persistence() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-large",
        "browser.capture",
        "completed",
        &"output".repeat(20_000),
        [("structured_output".to_string(), "structured".repeat(20_000))]
            .into_iter()
            .collect(),
        None,
    )
    .expect("tool event should append");

    let event = store
        .list_by_task(&phase16_task_id())
        .expect("events should load")
        .pop()
        .expect("tool event should exist");
    assert!(event.metadata["output"].len() <= PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES + 3);
    assert_eq!(event.metadata["output_omitted"], "true");
    assert_eq!(event.metadata["result_structured_output_omitted"], "true");
    assert!(event.metadata["result_structured_output"].starts_with("[omitted:"));
}

#[test]
fn persisted_tool_event_compaction_rewrites_legacy_payloads() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    store
        .append(Event {
            id: EventId("legacy-large-tool-event".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::ToolCallFinished,
            summary: "legacy tool output".to_string(),
            metadata: [
                ("session_id".to_string(), "session-a".to_string()),
                ("output".to_string(), "legacy-output".repeat(30_000)),
            ]
            .into_iter()
            .collect(),
        })
        .expect("legacy event should append");

    assert_eq!(
        compact_persisted_tool_event_metadata(&mut store).expect("legacy metadata should compact"),
        1
    );
    let event = store
        .event_by_id("legacy-large-tool-event")
        .expect("event should load")
        .expect("event should exist");
    assert_eq!(event.metadata["session_id"], "session-a");
    assert_eq!(event.metadata["output_omitted"], "true");
    assert!(event.metadata["output"].len() <= PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES + 3);
}
