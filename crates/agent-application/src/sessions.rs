#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTitleState {
    Pending,
    Automatic,
    Manual,
}

impl SessionTitleState {
    pub fn parse(value: Option<&str>, automatic_name: bool) -> Self {
        match value.map(str::trim) {
            Some("pending") => Self::Pending,
            Some("automatic") => Self::Automatic,
            Some("manual") => Self::Manual,
            _ if automatic_name => Self::Pending,
            _ => Self::Manual,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Automatic => "automatic",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLifecycleInput<'a> {
    pub status: &'a str,
    pub can_continue: bool,
    pub latest_sequence: u64,
    pub seen_event_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLifecycleProjection {
    pub activity: &'static str,
    pub attention_reason: Option<&'static str>,
    pub unseen_result: bool,
    pub status_label: &'static str,
    pub latest_sequence: u64,
}

pub fn project_session_lifecycle(input: SessionLifecycleInput<'_>) -> SessionLifecycleProjection {
    let unseen = input.latest_sequence > input.seen_event_sequence;
    match input.status {
        "running" => SessionLifecycleProjection {
            activity: "working",
            attention_reason: None,
            unseen_result: false,
            status_label: "Working",
            latest_sequence: input.latest_sequence,
        },
        "waiting_for_permission" => SessionLifecycleProjection {
            activity: "attention",
            attention_reason: Some("permission"),
            unseen_result: false,
            status_label: "Approval required",
            latest_sequence: input.latest_sequence,
        },
        "failed" if unseen => SessionLifecycleProjection {
            activity: "attention",
            attention_reason: Some("failed"),
            unseen_result: true,
            status_label: "Needs attention",
            latest_sequence: input.latest_sequence,
        },
        "paused" if input.can_continue => SessionLifecycleProjection {
            activity: "attention",
            attention_reason: Some("continuation"),
            unseen_result: false,
            status_label: "Continue",
            latest_sequence: input.latest_sequence,
        },
        "completed" if unseen => SessionLifecycleProjection {
            activity: "complete",
            attention_reason: None,
            unseen_result: true,
            status_label: "Completed",
            latest_sequence: input.latest_sequence,
        },
        _ => SessionLifecycleProjection {
            activity: "idle",
            attention_reason: None,
            unseen_result: false,
            status_label: "Ready",
            latest_sequence: input.latest_sequence,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(status: &str, seen: u64) -> SessionLifecycleProjection {
        project_session_lifecycle(SessionLifecycleInput {
            status,
            can_continue: status == "paused",
            latest_sequence: 12,
            seen_event_sequence: seen,
        })
    }

    #[test]
    fn legacy_title_records_preserve_manually_named_sessions() {
        assert_eq!(
            SessionTitleState::parse(None, true),
            SessionTitleState::Pending
        );
        assert_eq!(
            SessionTitleState::parse(None, false),
            SessionTitleState::Manual
        );
        assert_eq!(
            SessionTitleState::parse(Some("automatic"), false),
            SessionTitleState::Automatic
        );
    }

    #[test]
    fn terminal_statuses_only_request_attention_when_actionable_and_unseen() {
        assert_eq!(project("cancelled", 0).activity, "idle");
        assert_eq!(project("completed", 11).activity, "complete");
        assert_eq!(project("completed", 12).activity, "idle");
        assert_eq!(project("failed", 11).attention_reason, Some("failed"));
        assert_eq!(project("failed", 12).activity, "idle");
    }

    #[test]
    fn permission_and_continuation_remain_actionable() {
        assert_eq!(
            project("waiting_for_permission", 12).attention_reason,
            Some("permission")
        );
        assert_eq!(project("paused", 12).attention_reason, Some("continuation"));
    }
}
