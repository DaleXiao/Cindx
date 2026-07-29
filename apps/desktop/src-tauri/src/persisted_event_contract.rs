use agent_core::{insert_event_type_v1, EventKind, EventTypeV1, Metadata, EVENT_TYPE_METADATA_KEY};

pub(crate) fn tag_persisted_event_v1(kind: &EventKind, summary: &str, metadata: &mut Metadata) {
    if metadata.contains_key(EVENT_TYPE_METADATA_KEY) {
        return;
    }
    if let Some(event_type) = inferred_event_type_v1(kind, summary, metadata) {
        insert_event_type_v1(kind, metadata, event_type)
            .expect("inferred event type must match its event kind");
    }
}

fn inferred_event_type_v1(
    kind: &EventKind,
    summary: &str,
    metadata: &Metadata,
) -> Option<EventTypeV1> {
    match kind {
        EventKind::TaskStatusChanged => infer_status_type(summary, metadata),
        EventKind::ModelRequestStarted if summary == "Agent model turn started" => {
            Some(EventTypeV1::AgentModelTurnStarted)
        }
        EventKind::ModelRequestFinished if summary == "Agent model turn finished" => {
            Some(EventTypeV1::AgentModelTurnFinished)
        }
        EventKind::Error if summary == "Agent task failed" => Some(EventTypeV1::AgentRunFailed),
        EventKind::Error => Some(EventTypeV1::ErrorRecorded),
        _ => None,
    }
}

fn infer_status_type(summary: &str, metadata: &Metadata) -> Option<EventTypeV1> {
    if let Some(action) = metadata.get("queue_action").map(String::as_str) {
        return match action {
            "enqueue" => Some(EventTypeV1::AgentQueueEnqueued),
            "edit" => Some(EventTypeV1::AgentQueueEdited),
            "steer" => Some(EventTypeV1::AgentQueueSteerRequested),
            "delete" => Some(EventTypeV1::AgentQueueDeleted),
            "start" => Some(EventTypeV1::AgentQueueStarted),
            "restore" => Some(EventTypeV1::AgentQueueRestored),
            _ => None,
        };
    }
    match summary {
        "Agent task started" => Some(EventTypeV1::AgentRunStarted),
        "Agent task retry started" => Some(EventTypeV1::AgentRunRetryStarted),
        "Agent task waiting for permission" => Some(EventTypeV1::AgentRunWaitingForPermission),
        "Agent task resumed after permission" => Some(EventTypeV1::AgentRunResumedAfterPermission),
        "Agent task paused" => Some(EventTypeV1::AgentRunPaused),
        "Agent task completed" => Some(EventTypeV1::AgentRunCompleted),
        "Agent task cancelled" => Some(EventTypeV1::AgentRunCancelled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_read_model::agent_state_from_events;
    use crate::event_persistence::append_event;
    use crate::project_session_persistence::metadata_with_context;
    use agent_core::{Event, EventId, TaskId};
    use agent_storage::{EventStore, SqliteStore};

    #[test]
    fn central_writer_preserves_legacy_fields_and_future_tags() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-typed".to_string());
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("existing".to_string(), "kept".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("event should append");
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            [(
                EVENT_TYPE_METADATA_KEY.to_string(),
                "cindx.event.v2/agent.run.completed".to_string(),
            )]
            .into_iter()
            .collect(),
        )
        .expect("future event should append");

        let events = store.list_by_task(&task_id).expect("events should load");
        assert_eq!(events[0].summary, "Agent task started");
        assert_eq!(
            events[0].metadata.get("existing").map(String::as_str),
            Some("kept")
        );
        assert_eq!(
            events[0]
                .metadata
                .get(EVENT_TYPE_METADATA_KEY)
                .map(String::as_str),
            Some(EventTypeV1::AgentRunStarted.id())
        );
        assert_eq!(
            events[1]
                .metadata
                .get(EVENT_TYPE_METADATA_KEY)
                .map(String::as_str),
            Some("cindx.event.v2/agent.run.completed")
        );
    }

    #[test]
    fn event_type_is_local_when_context_is_reused() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-context-type".to_string());
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task started",
            Metadata::new(),
        )
        .expect("start should append");
        let start = store
            .list_by_task(&task_id)
            .expect("start should load")
            .pop()
            .expect("start should exist");
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task paused",
            metadata_with_context(Metadata::new(), &start.metadata),
        )
        .expect("pause should append");

        let future_context = [(
            EVENT_TYPE_METADATA_KEY.to_string(),
            "cindx.event.v2/agent.run.completed".to_string(),
        )]
        .into_iter()
        .collect();
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            metadata_with_context(Metadata::new(), &future_context),
        )
        .expect("completion should append");
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            future_context,
        )
        .expect("explicit future event should append");

        let events = store.list_by_task(&task_id).expect("events should load");
        assert_eq!(
            events[1]
                .metadata
                .get(EVENT_TYPE_METADATA_KEY)
                .map(String::as_str),
            Some(EventTypeV1::AgentRunPaused.id())
        );
        assert_eq!(
            events[2]
                .metadata
                .get(EVENT_TYPE_METADATA_KEY)
                .map(String::as_str),
            Some(EventTypeV1::AgentRunCompleted.id())
        );
        assert_eq!(
            events[3]
                .metadata
                .get(EVENT_TYPE_METADATA_KEY)
                .map(String::as_str),
            Some("cindx.event.v2/agent.run.completed")
        );
    }

    #[test]
    fn queue_tag_wins_over_lifecycle_like_summary() {
        let metadata = [("queue_action".to_string(), "delete".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            inferred_event_type_v1(
                &EventKind::TaskStatusChanged,
                "Agent task cancelled",
                &metadata
            ),
            Some(EventTypeV1::AgentQueueDeleted)
        );
    }

    #[test]
    fn model_request_tag_does_not_claim_non_agent_work() {
        let run_metadata = [("agent_run_id".to_string(), "run-a".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            inferred_event_type_v1(
                &EventKind::ModelRequestStarted,
                "RAG request started",
                &run_metadata,
            ),
            None
        );
        assert_eq!(
            inferred_event_type_v1(
                &EventKind::ModelRequestStarted,
                "Agent model turn started",
                &Metadata::new(),
            ),
            Some(EventTypeV1::AgentModelTurnStarted)
        );
    }

    #[test]
    fn event_kind_already_carries_plain_event_semantics() {
        for kind in [
            EventKind::TaskCreated,
            EventKind::MessageAdded,
            EventKind::ToolCallProposed,
            EventKind::ToolCallStarted,
            EventKind::ToolCallFinished,
            EventKind::PermissionRequested,
            EventKind::PermissionResolved,
            EventKind::RetrievalPerformed,
        ] {
            assert_eq!(
                inferred_event_type_v1(&kind, "display text", &Metadata::new()),
                None
            );
        }
    }

    #[test]
    fn read_model_separates_recorded_errors_from_forced_failure() {
        let store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("agent".to_string());
        let mut started_metadata = [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect();
        insert_event_type_v1(
            &EventKind::TaskStatusChanged,
            &mut started_metadata,
            EventTypeV1::AgentRunStarted,
        )
        .expect("start tag should build");
        let mut error_metadata = [
            ("session_id".to_string(), "session-a".to_string()),
            ("error".to_string(), "recoverable warning".to_string()),
        ]
        .into_iter()
        .collect();
        insert_event_type_v1(
            &EventKind::Error,
            &mut error_metadata,
            EventTypeV1::ErrorRecorded,
        )
        .expect("error tag should build");
        let events = vec![
            Event {
                id: EventId("start".to_string()),
                task_id: task_id.clone(),
                sequence: 1,
                timestamp_ms: 1,
                kind: EventKind::TaskStatusChanged,
                summary: "Agent task started".to_string(),
                metadata: started_metadata,
            },
            Event {
                id: EventId("warning".to_string()),
                task_id,
                sequence: 2,
                timestamp_ms: 2,
                kind: EventKind::Error,
                summary: "Background extraction unavailable".to_string(),
                metadata: error_metadata,
            },
        ];

        let recorded = agent_state_from_events(&store, None, Some("session-a"), events.clone())
            .expect("recorded error state should project");
        assert_eq!(recorded.status, "running");
        assert_eq!(recorded.last_error.as_deref(), Some("recoverable warning"));

        let forced = agent_state_from_events(
            &store,
            Some("forced failure".to_string()),
            Some("session-a"),
            events,
        )
        .expect("forced error state should project");
        assert_eq!(forced.status, "failed");
        assert_eq!(forced.last_error.as_deref(), Some("forced failure"));
    }
}
