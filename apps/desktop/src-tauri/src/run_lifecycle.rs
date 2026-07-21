use agent_core::{Event, EventKind};
use agent_runtime::AgentRunControl;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

pub(crate) struct RegisteredRunControl<'a> {
    controls: &'a Mutex<BTreeMap<String, Arc<AgentRunControl>>>,
    key: String,
    control: Arc<AgentRunControl>,
}

impl<'a> RegisteredRunControl<'a> {
    pub(crate) fn register(
        controls: &'a Mutex<BTreeMap<String, Arc<AgentRunControl>>>,
        key: impl Into<String>,
        control: Arc<AgentRunControl>,
        lock_label: &str,
        duplicate_error: &str,
    ) -> Result<Self, String> {
        let key = key.into();
        let mut entries = controls
            .lock()
            .map_err(|error| format!("{lock_label} lock poisoned: {error}"))?;
        if entries.contains_key(&key) {
            return Err(duplicate_error.to_string());
        }
        entries.insert(key.clone(), Arc::clone(&control));
        drop(entries);
        Ok(Self {
            controls,
            key,
            control,
        })
    }

    pub(crate) fn control(&self) -> Arc<AgentRunControl> {
        Arc::clone(&self.control)
    }
}

impl Drop for RegisteredRunControl<'_> {
    fn drop(&mut self) {
        let mut controls = self
            .controls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if controls
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.control))
        {
            controls.remove(&self.key);
        }
    }
}

pub(crate) struct ExclusiveKeyLease<'a> {
    keys: &'a Mutex<BTreeSet<String>>,
    key: String,
}

impl<'a> ExclusiveKeyLease<'a> {
    pub(crate) fn try_acquire(
        keys: &'a Mutex<BTreeSet<String>>,
        key: impl Into<String>,
        lock_label: &str,
    ) -> Result<Option<Self>, String> {
        let key = key.into();
        let mut entries = keys
            .lock()
            .map_err(|error| format!("{lock_label} lock poisoned: {error}"))?;
        if !entries.insert(key.clone()) {
            return Ok(None);
        }
        drop(entries);
        Ok(Some(Self { keys, key }))
    }
}

impl Drop for ExclusiveKeyLease<'_> {
    fn drop(&mut self) {
        self.keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}

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
        if has_error || events.iter().any(|event| event.kind == EventKind::Error) {
            return Self::Failed;
        }

        if let Some(status) = events
            .iter()
            .rev()
            .find_map(|event| AgentRunEvent::from_event(event).map(AgentRunEvent::status))
        {
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
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl AgentRunEvent {
    pub(crate) fn from_event(event: &Event) -> Option<Self> {
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
            "Agent task paused" => Some(Self::Paused),
            "Agent task completed" => Some(Self::Completed),
            "Agent task failed" => Some(Self::Failed),
            "Agent task cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub(crate) fn status(self) -> AgentRunStatus {
        match self {
            Self::Started | Self::RetryStarted => AgentRunStatus::Running,
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, Metadata, TaskId};

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

    #[test]
    fn lifecycle_events_define_one_status_vocabulary() {
        let cases = [
            ("Agent task started", AgentRunStatus::Running),
            ("Agent task retry started", AgentRunStatus::Running),
            (
                "Agent task waiting for permission",
                AgentRunStatus::WaitingForPermission,
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

    #[test]
    fn registered_run_control_is_removed_when_scope_ends() {
        let controls = Mutex::new(BTreeMap::new());
        let control = Arc::new(AgentRunControl::new("auto"));
        {
            let lease = RegisteredRunControl::register(
                &controls,
                "session-a",
                Arc::clone(&control),
                "test controls",
                "duplicate",
            )
            .expect("control should register");
            assert!(Arc::ptr_eq(&lease.control(), &control));
            assert!(controls.lock().unwrap().contains_key("session-a"));
        }
        assert!(!controls.lock().unwrap().contains_key("session-a"));
    }

    #[test]
    fn exclusive_key_can_be_reacquired_after_scope_ends() {
        let keys = Mutex::new(BTreeSet::new());
        let lease = ExclusiveKeyLease::try_acquire(&keys, "session-a", "test keys")
            .expect("key lock should work")
            .expect("first lease should acquire");
        assert!(
            ExclusiveKeyLease::try_acquire(&keys, "session-a", "test keys")
                .expect("key lock should work")
                .is_none()
        );
        drop(lease);
        assert!(
            ExclusiveKeyLease::try_acquire(&keys, "session-a", "test keys")
                .expect("key lock should work")
                .is_some()
        );
    }
}
