use super::*;
use crate::semantic_memory_runtime::contains_completed_agent_run;
use agent_core::{EventTypeV1, EVENT_TYPE_METADATA_KEY};

#[test]
fn goal2_semantic_memory_keeps_user_intent_lineage_and_only_terminal_epoch_outputs() {
    let base = [
        ("agent_run_id".to_string(), "memory-replanned".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let event = |sequence: u64, summary: &str, role: Option<&str>, epoch: &str| Event {
        id: EventId(format!("memory-epoch-event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: if role.is_some() {
            EventKind::MessageAdded
        } else {
            EventKind::TaskStatusChanged
        },
        summary: summary.to_string(),
        metadata: metadata_with_context(
            [
                ("steer_epoch".to_string(), epoch.to_string()),
                ("role".to_string(), role.unwrap_or_default().to_string()),
                ("content".to_string(), format!("objective epoch {epoch}")),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    };
    let events = vec![
        event(1, "Old user objective", Some("user"), "0"),
        event(2, "Old assistant result", Some("assistant"), "0"),
        event(3, "Revised user objective", Some("user"), "1"),
        event(4, "Revised assistant result", Some("assistant"), "1"),
        event(5, "Agent task completed", None, "1"),
    ];

    let filtered = memory_events_for_terminal_steer_epoch(events);
    assert_eq!(filtered.len(), 4);
    assert!(filtered.iter().any(|event| {
        event.summary == "Old user objective"
            && event.metadata.get("steer_epoch").map(String::as_str) == Some("0")
    }));
    assert!(filtered.iter().any(|event| {
        event.summary == "Revised user objective"
            && event.metadata.get("steer_epoch").map(String::as_str) == Some("1")
    }));
    assert!(!filtered
        .iter()
        .any(|event| event.summary == "Old assistant result"));
}

#[test]
fn goal2_semantic_memory_preserves_legacy_runs_without_epochs() {
    let events = vec![Event {
        id: EventId("legacy-complete".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 10,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task completed".to_string(),
        metadata: Metadata::new(),
    }];
    assert_eq!(memory_events_for_terminal_steer_epoch(events).len(), 1);
}

#[test]
fn memory_checkpoints_reject_future_and_kind_mismatched_event_tags() {
    let event = |kind: EventKind, summary: &str, event_type: Option<&str>| Event {
        id: EventId(format!("memory-contract-{summary}")),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 10,
        kind,
        summary: summary.to_string(),
        metadata: event_type
            .map(|event_type| {
                [(EVENT_TYPE_METADATA_KEY.to_string(), event_type.to_string())]
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default(),
    };

    assert!(is_memory_checkpoint_event(&event(
        EventKind::TaskStatusChanged,
        "Agent task completed",
        None,
    )));
    assert!(!is_memory_checkpoint_event(&event(
        EventKind::TaskStatusChanged,
        "Agent task completed",
        Some("cindx.event.v2/agent.run.completed"),
    )));
    assert!(!is_memory_checkpoint_event(&event(
        EventKind::MessageAdded,
        "Agent task completed",
        Some(EventTypeV1::AgentRunCompleted.id()),
    )));
    assert!(!is_memory_checkpoint_event(&event(
        EventKind::TaskStatusChanged,
        "Semantic memory candidates accepted",
        Some("cindx.event.v2/memory.candidates.accepted"),
    )));

    assert!(contains_completed_agent_run(&[event(
        EventKind::TaskStatusChanged,
        "Agent task completed",
        None,
    )]));
    assert!(!contains_completed_agent_run(&[event(
        EventKind::TaskStatusChanged,
        "Agent task completed",
        Some("cindx.event.v2/agent.run.completed"),
    )]));
    assert!(!contains_completed_agent_run(&[event(
        EventKind::MessageAdded,
        "Agent task completed",
        Some(EventTypeV1::AgentRunCompleted.id()),
    )]));
}
