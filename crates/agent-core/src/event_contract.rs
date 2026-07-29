use crate::{Event, EventKind, Metadata};
use std::fmt;

pub const EVENT_TYPE_METADATA_KEY: &str = "event_type";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventTypeV1 {
    AgentRunStarted,
    AgentRunRetryStarted,
    AgentRunWaitingForPermission,
    AgentRunResumedAfterPermission,
    AgentRunPaused,
    AgentRunCompleted,
    AgentRunFailed,
    AgentRunCancelled,
    AgentModelTurnStarted,
    AgentModelTurnFinished,
    AgentQueueEnqueued,
    AgentQueueEdited,
    AgentQueueSteerRequested,
    AgentQueueDeleted,
    AgentQueueStarted,
    AgentQueueRestored,
    ErrorRecorded,
}

impl EventTypeV1 {
    pub const fn id(self) -> &'static str {
        match self {
            Self::AgentRunStarted => "cindx.event.v1/agent.run.started",
            Self::AgentRunRetryStarted => "cindx.event.v1/agent.run.retry_started",
            Self::AgentRunWaitingForPermission => "cindx.event.v1/agent.run.waiting_for_permission",
            Self::AgentRunResumedAfterPermission => {
                "cindx.event.v1/agent.run.resumed_after_permission"
            }
            Self::AgentRunPaused => "cindx.event.v1/agent.run.paused",
            Self::AgentRunCompleted => "cindx.event.v1/agent.run.completed",
            Self::AgentRunFailed => "cindx.event.v1/agent.run.failed",
            Self::AgentRunCancelled => "cindx.event.v1/agent.run.cancelled",
            Self::AgentModelTurnStarted => "cindx.event.v1/agent.model_turn.started",
            Self::AgentModelTurnFinished => "cindx.event.v1/agent.model_turn.finished",
            Self::AgentQueueEnqueued => "cindx.event.v1/agent.queue.enqueued",
            Self::AgentQueueEdited => "cindx.event.v1/agent.queue.edited",
            Self::AgentQueueSteerRequested => "cindx.event.v1/agent.queue.steer_requested",
            Self::AgentQueueDeleted => "cindx.event.v1/agent.queue.deleted",
            Self::AgentQueueStarted => "cindx.event.v1/agent.queue.started",
            Self::AgentQueueRestored => "cindx.event.v1/agent.queue.restored",
            Self::ErrorRecorded => "cindx.event.v1/error.recorded",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "cindx.event.v1/agent.run.started" => Some(Self::AgentRunStarted),
            "cindx.event.v1/agent.run.retry_started" => Some(Self::AgentRunRetryStarted),
            "cindx.event.v1/agent.run.waiting_for_permission" => {
                Some(Self::AgentRunWaitingForPermission)
            }
            "cindx.event.v1/agent.run.resumed_after_permission" => {
                Some(Self::AgentRunResumedAfterPermission)
            }
            "cindx.event.v1/agent.run.paused" => Some(Self::AgentRunPaused),
            "cindx.event.v1/agent.run.completed" => Some(Self::AgentRunCompleted),
            "cindx.event.v1/agent.run.failed" => Some(Self::AgentRunFailed),
            "cindx.event.v1/agent.run.cancelled" => Some(Self::AgentRunCancelled),
            "cindx.event.v1/agent.model_turn.started" => Some(Self::AgentModelTurnStarted),
            "cindx.event.v1/agent.model_turn.finished" => Some(Self::AgentModelTurnFinished),
            "cindx.event.v1/agent.queue.enqueued" => Some(Self::AgentQueueEnqueued),
            "cindx.event.v1/agent.queue.edited" => Some(Self::AgentQueueEdited),
            "cindx.event.v1/agent.queue.steer_requested" => Some(Self::AgentQueueSteerRequested),
            "cindx.event.v1/agent.queue.deleted" => Some(Self::AgentQueueDeleted),
            "cindx.event.v1/agent.queue.started" => Some(Self::AgentQueueStarted),
            "cindx.event.v1/agent.queue.restored" => Some(Self::AgentQueueRestored),
            "cindx.event.v1/error.recorded" => Some(Self::ErrorRecorded),
            _ => None,
        }
    }

    pub fn matches_kind(self, kind: &EventKind) -> bool {
        match self {
            Self::AgentRunStarted
            | Self::AgentRunRetryStarted
            | Self::AgentRunWaitingForPermission
            | Self::AgentRunResumedAfterPermission
            | Self::AgentRunPaused
            | Self::AgentRunCompleted
            | Self::AgentRunCancelled
            | Self::AgentQueueEnqueued
            | Self::AgentQueueEdited
            | Self::AgentQueueSteerRequested
            | Self::AgentQueueDeleted
            | Self::AgentQueueStarted
            | Self::AgentQueueRestored => matches!(kind, EventKind::TaskStatusChanged),
            Self::AgentRunFailed | Self::ErrorRecorded => matches!(kind, EventKind::Error),
            Self::AgentModelTurnStarted => matches!(kind, EventKind::ModelRequestStarted),
            Self::AgentModelTurnFinished => matches!(kind, EventKind::ModelRequestFinished),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedEventRef<'a> {
    event_type: EventTypeV1,
    event: &'a Event,
}

impl<'a> TypedEventRef<'a> {
    pub const fn event_type(self) -> EventTypeV1 {
        self.event_type
    }

    pub const fn event(self) -> &'a Event {
        self.event
    }

    pub fn metadata(self, key: &str) -> Option<&'a str> {
        self.event.metadata.get(key).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedEventType<'a> {
    Legacy,
    V1(TypedEventRef<'a>),
    Invalid(&'a str),
}

pub fn decode_event_type(event: &Event) -> DecodedEventType<'_> {
    let Some(raw) = event
        .metadata
        .get(EVENT_TYPE_METADATA_KEY)
        .map(String::as_str)
    else {
        return DecodedEventType::Legacy;
    };
    let Some(event_type) = EventTypeV1::from_id(raw) else {
        return DecodedEventType::Invalid(raw);
    };
    if !event_type.matches_kind(&event.kind) {
        return DecodedEventType::Invalid(raw);
    }
    DecodedEventType::V1(TypedEventRef { event_type, event })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventTypeBuildError {
    KindMismatch(EventTypeV1),
    ReservedKeyOccupied,
}

impl fmt::Display for EventTypeBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KindMismatch(event_type) => write!(
                formatter,
                "event type `{}` does not match the event kind",
                event_type.id()
            ),
            Self::ReservedKeyOccupied => {
                write!(
                    formatter,
                    "metadata key `{EVENT_TYPE_METADATA_KEY}` is reserved"
                )
            }
        }
    }
}

impl std::error::Error for EventTypeBuildError {}

pub fn insert_event_type_v1(
    kind: &EventKind,
    metadata: &mut Metadata,
    event_type: EventTypeV1,
) -> Result<(), EventTypeBuildError> {
    if !event_type.matches_kind(kind) {
        return Err(EventTypeBuildError::KindMismatch(event_type));
    }
    match metadata.get(EVENT_TYPE_METADATA_KEY) {
        Some(existing) if existing == event_type.id() => Ok(()),
        Some(_) => Err(EventTypeBuildError::ReservedKeyOccupied),
        None => {
            metadata.insert(
                EVENT_TYPE_METADATA_KEY.to_string(),
                event_type.id().to_string(),
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventId, TaskId};

    fn event(kind: EventKind, metadata: Metadata) -> Event {
        Event {
            id: EventId("event-1".to_string()),
            task_id: TaskId("task-1".to_string()),
            sequence: 1,
            timestamp_ms: 1,
            kind,
            summary: "summary may change".to_string(),
            metadata,
        }
    }

    #[test]
    fn v1_ids_round_trip_without_allocating_on_read() {
        let event_types = [
            EventTypeV1::AgentRunStarted,
            EventTypeV1::AgentRunResumedAfterPermission,
            EventTypeV1::AgentModelTurnFinished,
            EventTypeV1::AgentQueueRestored,
            EventTypeV1::ErrorRecorded,
        ];
        for event_type in event_types {
            assert_eq!(EventTypeV1::from_id(event_type.id()), Some(event_type));
        }
    }

    #[test]
    fn builder_rejects_kind_mismatches_and_reserved_key_collisions() {
        let mut metadata = Metadata::new();
        assert_eq!(
            insert_event_type_v1(
                &EventKind::MessageAdded,
                &mut metadata,
                EventTypeV1::AgentRunStarted,
            ),
            Err(EventTypeBuildError::KindMismatch(
                EventTypeV1::AgentRunStarted
            ))
        );
        metadata.insert(
            EVENT_TYPE_METADATA_KEY.to_string(),
            "cindx.event.v2/agent.run.started".to_string(),
        );
        assert_eq!(
            insert_event_type_v1(
                &EventKind::TaskStatusChanged,
                &mut metadata,
                EventTypeV1::AgentRunStarted,
            ),
            Err(EventTypeBuildError::ReservedKeyOccupied)
        );
    }

    #[test]
    fn decoder_distinguishes_legacy_typed_and_invalid_events() {
        assert!(matches!(
            decode_event_type(&event(EventKind::TaskStatusChanged, Metadata::new())),
            DecodedEventType::Legacy
        ));

        let mut metadata = Metadata::new();
        insert_event_type_v1(
            &EventKind::TaskStatusChanged,
            &mut metadata,
            EventTypeV1::AgentRunCompleted,
        )
        .expect("matching type should build");
        let typed = event(EventKind::TaskStatusChanged, metadata);
        assert!(matches!(
            decode_event_type(&typed),
            DecodedEventType::V1(value)
                if value.event_type() == EventTypeV1::AgentRunCompleted
                    && value.event().summary == "summary may change"
        ));

        let invalid = event(
            EventKind::MessageAdded,
            [(
                EVENT_TYPE_METADATA_KEY.to_string(),
                EventTypeV1::AgentRunCompleted.id().to_string(),
            )]
            .into_iter()
            .collect(),
        );
        assert!(matches!(
            decode_event_type(&invalid),
            DecodedEventType::Invalid(value) if value == EventTypeV1::AgentRunCompleted.id()
        ));
    }
}
