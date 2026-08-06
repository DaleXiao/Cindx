use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRecoveryState {
    Paused,
    Blocked,
    Resuming,
    Unknown(String),
}

impl AgentRecoveryState {
    pub fn parse(value: &str) -> Self {
        match value {
            "paused" => Self::Paused,
            "blocked" => Self::Blocked,
            "resuming" => Self::Resuming,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::Resuming => "resuming",
            Self::Unknown(value) => value,
        }
    }
}

impl Serialize for AgentRecoveryState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for AgentRecoveryState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::parse(&value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRecoveryReason {
    AppRestarted,
    AppRestartedWaitingForPermission,
    WaitingForPermission,
    PermissionResolved,
    UserContinued,
    UserCancelled,
    DeadlineExceeded,
    ModelCallBudgetExceeded,
    ToolCallBudgetExceeded,
    TurnBudgetExhausted,
    ProviderUnavailable,
    NoProgress,
    RepeatedAction,
    StageBudgetExhausted,
    RepairBudgetExhausted,
    ModelResourceBudgetExceeded,
    Unknown(String),
}

impl AgentRecoveryReason {
    pub fn parse(value: &str) -> Self {
        match value {
            "app_restarted" => Self::AppRestarted,
            "app_restarted_waiting_for_permission" => Self::AppRestartedWaitingForPermission,
            "waiting_for_permission" => Self::WaitingForPermission,
            "permission_resolved" => Self::PermissionResolved,
            "user_continued" => Self::UserContinued,
            "user_cancelled" => Self::UserCancelled,
            "deadline_exceeded" => Self::DeadlineExceeded,
            "model_call_budget_exceeded" => Self::ModelCallBudgetExceeded,
            "tool_call_budget_exceeded" => Self::ToolCallBudgetExceeded,
            "turn_budget_exhausted" => Self::TurnBudgetExhausted,
            "provider_unavailable" => Self::ProviderUnavailable,
            "no_progress" => Self::NoProgress,
            "repeated_action" => Self::RepeatedAction,
            "stage_budget_exhausted" => Self::StageBudgetExhausted,
            "repair_budget_exhausted" => Self::RepairBudgetExhausted,
            "model_resource_budget_exceeded" => Self::ModelResourceBudgetExceeded,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::AppRestarted => "app_restarted",
            Self::AppRestartedWaitingForPermission => "app_restarted_waiting_for_permission",
            Self::WaitingForPermission => "waiting_for_permission",
            Self::PermissionResolved => "permission_resolved",
            Self::UserContinued => "user_continued",
            Self::UserCancelled => "user_cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ModelCallBudgetExceeded => "model_call_budget_exceeded",
            Self::ToolCallBudgetExceeded => "tool_call_budget_exceeded",
            Self::TurnBudgetExhausted => "turn_budget_exhausted",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::NoProgress => "no_progress",
            Self::RepeatedAction => "repeated_action",
            Self::StageBudgetExhausted => "stage_budget_exhausted",
            Self::RepairBudgetExhausted => "repair_budget_exhausted",
            Self::ModelResourceBudgetExceeded => "model_resource_budget_exceeded",
            Self::Unknown(value) => value,
        }
    }
}

impl Serialize for AgentRecoveryReason {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for AgentRecoveryReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::parse(&value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecoveryIdentity {
    pub project_id: Option<String>,
    pub session_id: String,
    pub resume_key: String,
    pub source_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_run_id: Option<String>,
    pub user_turn_sequence: u64,
    pub prompt_fingerprint: String,
}

impl AgentRecoveryIdentity {
    pub fn logical_run_id(&self) -> &str {
        self.logical_run_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(self.source_run_id.as_str())
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.session_id.trim().is_empty() {
            return Err("agent recovery identity is missing session_id");
        }
        if self.resume_key.trim().is_empty() {
            return Err("agent recovery identity is missing resume_key");
        }
        if self.prompt_fingerprint.trim().is_empty() {
            return Err("agent recovery identity is missing prompt_fingerprint");
        }
        if self
            .logical_run_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err("agent recovery identity has an empty logical_run_id");
        }
        Ok(())
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.validate().is_ok()
            && other.validate().is_ok()
            && self.project_id == other.project_id
            && self.session_id == other.session_id
            && self.resume_key == other.resume_key
            && self.source_run_id == other.source_run_id
            && match (&self.logical_run_id, &other.logical_run_id) {
                (Some(left), Some(right)) => left == right,
                _ => true,
            }
            && self.user_turn_sequence == other.user_turn_sequence
            && self.prompt_fingerprint == other.prompt_fingerprint
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAgentRecovery {
    pub identity: AgentRecoveryIdentity,
    pub prompt: String,
}

impl ResolvedAgentRecovery {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.identity.validate()?;
        if self.prompt.trim().is_empty() {
            return Err("resolved agent recovery is missing prompt");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> AgentRecoveryIdentity {
        AgentRecoveryIdentity {
            project_id: Some("project-1".to_string()),
            session_id: "session-1".to_string(),
            resume_key: "agent-resume-1".to_string(),
            source_run_id: "run-1".to_string(),
            logical_run_id: Some("logical-run-1".to_string()),
            user_turn_sequence: 42,
            prompt_fingerprint: "prompt-sha".to_string(),
        }
    }

    #[test]
    fn recovery_state_preserves_known_wire_values() {
        for (encoded, expected) in [
            ("paused", AgentRecoveryState::Paused),
            ("blocked", AgentRecoveryState::Blocked),
            ("resuming", AgentRecoveryState::Resuming),
        ] {
            let state: AgentRecoveryState =
                serde_json::from_str(&format!("\"{encoded}\"")).expect("state should decode");
            assert_eq!(state, expected);
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{encoded}\"")
            );
        }
    }

    #[test]
    fn recovery_reason_preserves_known_wire_values() {
        for encoded in [
            "app_restarted",
            "app_restarted_waiting_for_permission",
            "waiting_for_permission",
            "permission_resolved",
            "user_continued",
            "user_cancelled",
            "deadline_exceeded",
            "model_call_budget_exceeded",
            "tool_call_budget_exceeded",
            "turn_budget_exhausted",
            "provider_unavailable",
            "no_progress",
            "repeated_action",
            "stage_budget_exhausted",
            "repair_budget_exhausted",
            "model_resource_budget_exceeded",
        ] {
            let reason: AgentRecoveryReason =
                serde_json::from_str(&format!("\"{encoded}\"")).expect("reason should decode");
            assert_eq!(reason.label(), encoded);
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("\"{encoded}\"")
            );
        }
    }

    #[test]
    fn unknown_recovery_values_round_trip_for_legacy_compatibility() {
        let state: AgentRecoveryState =
            serde_json::from_str("\"legacy_suspended\"").expect("legacy state should decode");
        let reason: AgentRecoveryReason =
            serde_json::from_str("\"legacy_reason\"").expect("legacy reason should decode");

        assert_eq!(
            state,
            AgentRecoveryState::Unknown("legacy_suspended".to_string())
        );
        assert_eq!(
            reason,
            AgentRecoveryReason::Unknown("legacy_reason".to_string())
        );
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            "\"legacy_suspended\""
        );
        assert_eq!(serde_json::to_string(&reason).unwrap(), "\"legacy_reason\"");
    }

    #[test]
    fn recovery_identity_matches_only_the_same_scoped_turn() {
        let expected = identity();
        assert!(expected.matches(&identity()));

        let mut other_project = identity();
        other_project.project_id = Some("project-2".to_string());
        assert!(!expected.matches(&other_project));

        let mut other_session = identity();
        other_session.session_id = "session-2".to_string();
        assert!(!expected.matches(&other_session));

        let mut other_run = identity();
        other_run.source_run_id = "run-2".to_string();
        assert!(!expected.matches(&other_run));

        let mut other_logical_run = identity();
        other_logical_run.logical_run_id = Some("logical-run-2".to_string());
        assert!(!expected.matches(&other_logical_run));

        let mut other_turn = identity();
        other_turn.user_turn_sequence += 1;
        assert!(!expected.matches(&other_turn));

        let mut other_prompt = identity();
        other_prompt.prompt_fingerprint = "other-prompt-sha".to_string();
        assert!(!expected.matches(&other_prompt));
    }

    #[test]
    fn recovery_identity_rejects_missing_durable_scope() {
        let mut missing_session = identity();
        missing_session.session_id.clear();
        assert!(missing_session.validate().is_err());

        let mut missing_key = identity();
        missing_key.resume_key.clear();
        assert!(missing_key.validate().is_err());

        let mut missing_prompt = identity();
        missing_prompt.prompt_fingerprint.clear();
        assert!(missing_prompt.validate().is_err());

        let mut empty_logical_run = identity();
        empty_logical_run.logical_run_id = Some(String::new());
        assert!(empty_logical_run.validate().is_err());
    }

    #[test]
    fn legacy_recovery_identity_uses_source_run_as_its_logical_run() {
        let encoded = r#"{
            "projectId":"project-1",
            "sessionId":"session-1",
            "resumeKey":"agent-resume-1",
            "sourceRunId":"run-1",
            "userTurnSequence":42,
            "promptFingerprint":"prompt-sha"
        }"#;
        let legacy: AgentRecoveryIdentity =
            serde_json::from_str(encoded).expect("legacy identity should decode");
        let mut current = legacy.clone();
        current.logical_run_id = Some("logical-root-run".to_string());

        assert_eq!(legacy.logical_run_id, None);
        assert_eq!(legacy.logical_run_id(), "run-1");
        assert!(legacy.matches(&current));
        assert!(current.matches(&legacy));
    }

    #[test]
    fn resolved_prompt_is_runtime_only_and_required() {
        let resolved = ResolvedAgentRecovery {
            identity: identity(),
            prompt: "Continue the original objective".to_string(),
        };
        assert_eq!(resolved.validate(), Ok(()));
        assert!(!serde_json::to_string(&resolved.identity)
            .expect("identity should encode")
            .contains("Continue the original objective"));
    }
}
