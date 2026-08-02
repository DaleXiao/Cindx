use std::collections::BTreeMap;

mod event_contract;
mod permission_policy;

pub use event_contract::{
    decode_event_type, insert_event_type_v1, DecodedEventType, EventTypeBuildError, EventTypeV1,
    TypedEventRef, EVENT_TYPE_METADATA_KEY,
};
pub use permission_policy::{
    permission_can_allow_session, permission_capability_matches, permission_requires_exact_scope,
};

pub type Metadata = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolCallId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PermissionRequestId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    WaitingForPermission,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
    Reviewer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    TaskCreated,
    TaskStatusChanged,
    MessageAdded,
    ModelRequestStarted,
    ModelRequestFinished,
    ToolCallProposed,
    ToolCallStarted,
    ToolCallFinished,
    PermissionRequested,
    PermissionResolved,
    RetrievalPerformed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: EventId,
    pub task_id: TaskId,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub kind: EventKind,
    pub summary: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionRisk {
    Read,
    Write,
    Execute,
    Network,
    Sensitive,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    AllowForSession,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub id: PermissionRequestId,
    pub task_id: TaskId,
    pub risk: PermissionRisk,
    pub action: String,
    pub reason: String,
    pub scope: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionResolution {
    pub request_id: PermissionRequestId,
    pub decision: PermissionDecision,
    pub resolved_at_ms: u64,
    pub resolved_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRisk {
    ReadOnly,
    WritesWorkspace,
    ExecutesProcess,
    UsesNetwork,
    SensitiveContext,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    BuiltIn,
    Mcp { server_id: String },
    Skill { skill_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    Inline,
    Deferred,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEffectSemantics {
    ReadOnly,
    Idempotent,
    Verifiable { verifier: String },
    NonIdempotent,
}

impl ToolEffectSemantics {
    pub fn conservative_default(risk: &ToolRisk) -> Self {
        if matches!(risk, ToolRisk::ReadOnly) {
            Self::ReadOnly
        } else {
            Self::NonIdempotent
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Idempotent => "idempotent",
            Self::Verifiable { .. } => "verifiable",
            Self::NonIdempotent => "non_idempotent",
        }
    }

    pub fn verifier(&self) -> Option<&str> {
        match self {
            Self::Verifiable { verifier } => Some(verifier.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolExecutionConcurrency {
    #[default]
    Serialized,
    IndependentRead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub risk: ToolRisk,
    pub source: ToolSource,
    pub exposure: ToolExposure,
    pub input_schema_json: String,
    pub output_schema_json: Option<String>,
    pub effect_semantics: ToolEffectSemantics,
    pub execution_concurrency: ToolExecutionConcurrency,
}

impl ToolSpec {
    pub fn new(
        name: impl Into<String>,
        namespace: impl Into<String>,
        description: impl Into<String>,
        risk: ToolRisk,
        source: ToolSource,
        exposure: ToolExposure,
        input_schema_json: impl Into<String>,
    ) -> Self {
        let effect_semantics = ToolEffectSemantics::conservative_default(&risk);
        Self {
            name: name.into(),
            namespace: namespace.into(),
            description: description.into(),
            risk,
            source,
            exposure,
            input_schema_json: input_schema_json.into(),
            output_schema_json: None,
            effect_semantics,
            execution_concurrency: ToolExecutionConcurrency::default(),
        }
    }

    pub fn with_effect_semantics(mut self, effect_semantics: ToolEffectSemantics) -> Self {
        self.effect_semantics = effect_semantics;
        self
    }

    pub fn with_execution_concurrency(
        mut self,
        execution_concurrency: ToolExecutionConcurrency,
    ) -> Self {
        self.execution_concurrency = execution_concurrency;
        self
    }

    pub fn builtin(
        name: impl Into<String>,
        namespace: impl Into<String>,
        description: impl Into<String>,
        risk: ToolRisk,
        input_schema_json: impl Into<String>,
    ) -> Self {
        Self::new(
            name,
            namespace,
            description,
            risk,
            ToolSource::BuiltIn,
            ToolExposure::Auto,
            input_schema_json,
        )
    }

    pub fn validate_input_schema(&self) -> Result<(), String> {
        let value: serde_json::Value = serde_json::from_str(&self.input_schema_json)
            .map_err(|error| format!("invalid JSON schema for {}: {error}", self.name))?;
        if value.get("type").and_then(serde_json::Value::as_str) != Some("object") {
            return Err(format!(
                "tool {} input schema must describe an object",
                self.name
            ));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_input_schema()?;
        if matches!(
            self.execution_concurrency,
            ToolExecutionConcurrency::IndependentRead
        ) && (!matches!(self.risk, ToolRisk::ReadOnly)
            || !matches!(self.effect_semantics, ToolEffectSemantics::ReadOnly))
        {
            return Err(format!(
                "tool {} may use independent-read concurrency only with read-only risk and effect semantics",
                self.name
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    pub id: ToolCallId,
    pub task_id: TaskId,
    pub tool_name: String,
    pub input_json: String,
    pub proposed_by_model: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcomeStatus {
    Succeeded,
    Failed,
    Cancelled,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolContent {
    Text(String),
    Image { mime_type: String, data: String },
    Resource { uri: String, text: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolArtifact {
    pub path: String,
    pub mime_type: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub invocation_id: ToolCallId,
    pub status: ToolOutcomeStatus,
    pub output: String,
    pub content: Vec<ToolContent>,
    pub structured_output_json: Option<String>,
    pub artifacts: Vec<ToolArtifact>,
    pub failure: Option<ToolFailure>,
    pub metadata: Metadata,
}

impl ToolResult {
    pub fn text(
        invocation_id: ToolCallId,
        status: ToolOutcomeStatus,
        output: impl Into<String>,
        metadata: Metadata,
    ) -> Self {
        let output = output.into();
        let failure = if matches!(status, ToolOutcomeStatus::Failed) {
            Some(ToolFailure {
                code: "tool_execution_failed".to_string(),
                message: output.clone(),
                retryable: false,
            })
        } else {
            None
        };
        Self {
            invocation_id,
            status,
            content: vec![ToolContent::Text(output.clone())],
            output,
            structured_output_json: None,
            artifacts: Vec::new(),
            failure,
            metadata,
        }
    }

    pub fn failed(invocation_id: ToolCallId, error: impl Into<String>) -> Self {
        Self::text(
            invocation_id,
            ToolOutcomeStatus::Failed,
            error,
            Metadata::new(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRole {
    Planner,
    Executor,
    Reviewer,
    Summarizer,
    Embedder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProfile {
    pub provider: String,
    pub model: String,
    pub role: ModelRole,
    pub context_window: Option<u64>,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub metadata: Metadata,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(risk: ToolRisk) -> ToolSpec {
        ToolSpec::builtin(
            "test.tool",
            "test",
            "test tool",
            risk,
            r#"{"type":"object","properties":{}}"#,
        )
    }

    #[test]
    fn tool_specs_default_to_serial_execution() {
        assert_eq!(
            spec(ToolRisk::ReadOnly).execution_concurrency,
            ToolExecutionConcurrency::Serialized
        );
    }

    #[test]
    fn independent_read_concurrency_requires_read_only_risk_and_effect() {
        assert!(spec(ToolRisk::ReadOnly)
            .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
            .validate()
            .is_ok());

        let write_error = spec(ToolRisk::WritesWorkspace)
            .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
            .validate()
            .expect_err("write tools must remain serialized");
        assert!(write_error.contains("read-only risk and effect semantics"));

        let effect_error = spec(ToolRisk::ReadOnly)
            .with_effect_semantics(ToolEffectSemantics::Idempotent)
            .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
            .validate()
            .expect_err("non-read-only effects must remain serialized");
        assert!(effect_error.contains("read-only risk and effect semantics"));
    }
}
