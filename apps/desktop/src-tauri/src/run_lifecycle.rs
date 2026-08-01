use agent_core::{decode_event_type, DecodedEventType, Event, EventKind, EventTypeV1};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRunStatus {
    Idle,
    Running,
    WaitingForPermission,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl AgentRunStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::WaitingForPermission => "waiting_for_permission",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "waiting_for_permission" => Self::WaitingForPermission,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Idle,
        }
    }

    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub(crate) fn can_cancel(self) -> bool {
        matches!(self, Self::Running | Self::WaitingForPermission)
    }

    pub(crate) fn can_retry(self, has_user_prompt: bool) -> bool {
        has_user_prompt
            && matches!(
                self,
                Self::Paused | Self::Completed | Self::Failed | Self::Cancelled
            )
    }

    pub(crate) fn can_continue(self, partial_completion: bool) -> bool {
        self == Self::Paused || (self == Self::Completed && partial_completion)
    }

    pub(crate) fn from_events(
        events: &[Event],
        has_pending_approval: bool,
        has_error: bool,
    ) -> Self {
        if has_error {
            return Self::Failed;
        }

        let mut latest_status = None;
        for event in events {
            let Some(status) = AgentRunEvent::from_event(event).map(AgentRunEvent::status) else {
                continue;
            };
            if status == Self::Failed {
                return Self::Failed;
            }
            latest_status = Some(status);
        }

        if let Some(status) = latest_status {
            return if status == Self::Running && has_pending_approval {
                Self::WaitingForPermission
            } else {
                status
            };
        }

        if has_pending_approval {
            Self::WaitingForPermission
        } else if events.is_empty() {
            Self::Idle
        } else {
            Self::Running
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRunEvent {
    Started,
    RetryStarted,
    WaitingForPermission,
    ResumedAfterPermission,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl AgentRunEvent {
    pub(crate) fn from_event(event: &Event) -> Option<Self> {
        Self::try_from_event(event).ok().flatten()
    }

    pub(crate) fn try_from_event(event: &Event) -> Result<Option<Self>, ()> {
        if !matches!(&event.kind, EventKind::TaskStatusChanged | EventKind::Error) {
            return Ok(None);
        }
        match decode_event_type(event) {
            DecodedEventType::V1(typed) => Ok(Self::from_event_type(typed.event_type())),
            DecodedEventType::Invalid(_) => Err(()),
            DecodedEventType::Legacy => Ok(Self::from_legacy_event(event)),
        }
    }

    fn from_event_type(event_type: EventTypeV1) -> Option<Self> {
        match event_type {
            EventTypeV1::AgentRunStarted => Some(Self::Started),
            EventTypeV1::AgentRunRetryStarted => Some(Self::RetryStarted),
            EventTypeV1::AgentRunWaitingForPermission => Some(Self::WaitingForPermission),
            EventTypeV1::AgentRunResumedAfterPermission => Some(Self::ResumedAfterPermission),
            EventTypeV1::AgentRunPaused => Some(Self::Paused),
            EventTypeV1::AgentRunCompleted => Some(Self::Completed),
            EventTypeV1::AgentRunFailed => Some(Self::Failed),
            EventTypeV1::AgentRunCancelled => Some(Self::Cancelled),
            _ => None,
        }
    }

    fn from_legacy_event(event: &Event) -> Option<Self> {
        if event.metadata.contains_key("queue_action") {
            return None;
        }
        if event.kind == EventKind::Error {
            return Some(Self::Failed);
        }
        if event.kind != EventKind::TaskStatusChanged {
            return None;
        }
        Self::from_summary(&event.summary)
    }

    pub(crate) fn from_summary(summary: &str) -> Option<Self> {
        match summary {
            "Agent task started" => Some(Self::Started),
            "Agent task retry started" => Some(Self::RetryStarted),
            "Agent task waiting for permission" => Some(Self::WaitingForPermission),
            "Agent task resumed after permission" => Some(Self::ResumedAfterPermission),
            "Agent task paused" => Some(Self::Paused),
            "Agent task completed" => Some(Self::Completed),
            "Agent task failed" => Some(Self::Failed),
            "Agent task cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub(crate) fn status(self) -> AgentRunStatus {
        match self {
            Self::Started | Self::RetryStarted | Self::ResumedAfterPermission => {
                AgentRunStatus::Running
            }
            Self::WaitingForPermission => AgentRunStatus::WaitingForPermission,
            Self::Paused => AgentRunStatus::Paused,
            Self::Completed => AgentRunStatus::Completed,
            Self::Failed => AgentRunStatus::Failed,
            Self::Cancelled => AgentRunStatus::Cancelled,
        }
    }

    pub(crate) fn is_start(self) -> bool {
        matches!(self, Self::Started | Self::RetryStarted)
    }
}

fn is_agent_model_turn_event(
    event: &Event,
    kind: EventKind,
    event_type: EventTypeV1,
    legacy_summary: &str,
) -> bool {
    if event.kind != kind {
        return false;
    }
    match decode_event_type(event) {
        DecodedEventType::V1(typed) => typed.event_type() == event_type,
        DecodedEventType::Legacy => event.summary == legacy_summary,
        DecodedEventType::Invalid(_) => false,
    }
}

pub(crate) fn is_agent_model_turn_started(event: &Event) -> bool {
    is_agent_model_turn_event(
        event,
        EventKind::ModelRequestStarted,
        EventTypeV1::AgentModelTurnStarted,
        "Agent model turn started",
    )
}

pub(crate) fn is_agent_model_turn_finished(event: &Event) -> bool {
    is_agent_model_turn_event(
        event,
        EventKind::ModelRequestFinished,
        EventTypeV1::AgentModelTurnFinished,
        "Agent model turn finished",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{insert_event_type_v1, EventId, Metadata, TaskId, EVENT_TYPE_METADATA_KEY};

    fn event(sequence: u64, kind: EventKind, summary: &str) -> Event {
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: TaskId("agent".to_string()),
            sequence,
            timestamp_ms: sequence,
            kind,
            summary: summary.to_string(),
            metadata: Metadata::new(),
        }
    }

    fn typed_event(
        sequence: u64,
        kind: EventKind,
        summary: &str,
        event_type: EventTypeV1,
    ) -> Event {
        let mut event = event(sequence, kind, summary);
        insert_event_type_v1(&event.kind, &mut event.metadata, event_type)
            .expect("test event type should match its kind");
        event
    }

    #[test]
    fn lifecycle_events_define_one_status_vocabulary() {
        let cases = [
            ("Agent task started", AgentRunStatus::Running),
            ("Agent task retry started", AgentRunStatus::Running),
            (
                "Agent task waiting for permission",
                AgentRunStatus::WaitingForPermission,
            ),
            (
                "Agent task resumed after permission",
                AgentRunStatus::Running,
            ),
            ("Agent task paused", AgentRunStatus::Paused),
            ("Agent task completed", AgentRunStatus::Completed),
            ("Agent task failed", AgentRunStatus::Failed),
            ("Agent task cancelled", AgentRunStatus::Cancelled),
        ];
        for (summary, expected) in cases {
            assert_eq!(
                AgentRunEvent::from_summary(summary).map(|event| event.status()),
                Some(expected)
            );
        }
    }

    #[test]
    fn errors_win_over_stale_terminal_events() {
        let events = vec![
            event(1, EventKind::TaskStatusChanged, "Agent task started"),
            event(2, EventKind::TaskStatusChanged, "Agent task completed"),
            event(3, EventKind::Error, "Agent task failed"),
        ];
        assert_eq!(
            AgentRunStatus::from_events(&events, false, false),
            AgentRunStatus::Failed
        );
    }

    #[test]
    fn queue_events_do_not_change_run_status() {
        let mut queued = event(2, EventKind::TaskStatusChanged, "Agent task cancelled");
        queued
            .metadata
            .insert("queue_action".to_string(), "delete".to_string());
        let events = vec![
            event(1, EventKind::TaskStatusChanged, "Agent task started"),
            queued,
        ];
        assert_eq!(
            AgentRunStatus::from_events(&events, false, false),
            AgentRunStatus::Running
        );
    }

    #[test]
    fn typed_lifecycle_does_not_depend_on_display_summary() {
        let events = vec![typed_event(
            1,
            EventKind::TaskStatusChanged,
            "任务已完成",
            EventTypeV1::AgentRunCompleted,
        )];
        assert_eq!(
            AgentRunStatus::from_events(&events, false, false),
            AgentRunStatus::Completed
        );
    }

    #[test]
    fn mixed_replay_resumes_after_permission() {
        let events = vec![
            event(1, EventKind::TaskStatusChanged, "Agent task started"),
            typed_event(
                2,
                EventKind::TaskStatusChanged,
                "waiting display text",
                EventTypeV1::AgentRunWaitingForPermission,
            ),
            typed_event(
                3,
                EventKind::TaskStatusChanged,
                "resumed display text",
                EventTypeV1::AgentRunResumedAfterPermission,
            ),
        ];
        assert_eq!(
            AgentRunStatus::from_events(&events, false, false),
            AgentRunStatus::Running
        );
    }

    #[test]
    fn unknown_or_mismatched_tags_fail_closed() {
        let mut future = event(2, EventKind::TaskStatusChanged, "Agent task completed");
        future.metadata.insert(
            EVENT_TYPE_METADATA_KEY.to_string(),
            "cindx.event.v2/agent.run.completed".to_string(),
        );
        let mut mismatched = event(3, EventKind::MessageAdded, "Agent task completed");
        mismatched.metadata.insert(
            EVENT_TYPE_METADATA_KEY.to_string(),
            EventTypeV1::AgentRunCompleted.id().to_string(),
        );
        let events = vec![
            event(1, EventKind::TaskStatusChanged, "Agent task started"),
            future,
            mismatched,
        ];
        assert_eq!(
            AgentRunStatus::from_events(&events, false, false),
            AgentRunStatus::Running
        );
    }

    #[test]
    fn typed_queue_and_nonterminal_errors_cannot_spoof_lifecycle() {
        let events = vec![
            typed_event(
                1,
                EventKind::TaskStatusChanged,
                "Agent task started",
                EventTypeV1::AgentRunStarted,
            ),
            typed_event(
                2,
                EventKind::TaskStatusChanged,
                "Agent task cancelled",
                EventTypeV1::AgentQueueDeleted,
            ),
            typed_event(
                3,
                EventKind::Error,
                "Agent task failed",
                EventTypeV1::ErrorRecorded,
            ),
        ];
        assert_eq!(
            AgentRunStatus::from_events(&events, false, false),
            AgentRunStatus::Running
        );
    }

    #[test]
    fn pending_permission_overrides_a_running_projection() {
        let events = vec![event(1, EventKind::TaskStatusChanged, "Agent task started")];
        assert_eq!(
            AgentRunStatus::from_events(&events, true, false),
            AgentRunStatus::WaitingForPermission
        );
    }

    #[test]
    fn capabilities_follow_the_lifecycle() {
        assert!(AgentRunStatus::Running.can_cancel());
        assert!(AgentRunStatus::WaitingForPermission.can_cancel());
        assert!(AgentRunStatus::Paused.can_continue(false));
        assert!(AgentRunStatus::Completed.can_continue(true));
        assert!(!AgentRunStatus::Completed.can_continue(false));
        assert!(AgentRunStatus::Failed.can_retry(true));
        assert!(!AgentRunStatus::Failed.can_retry(false));
        assert!(AgentRunStatus::Completed.is_terminal());
    }

}
