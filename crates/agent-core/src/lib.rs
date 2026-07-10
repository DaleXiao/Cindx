use std::collections::BTreeMap;

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
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub risk: ToolRisk,
    pub input_schema_json: String,
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
pub struct ToolResult {
    pub invocation_id: ToolCallId,
    pub status: ToolOutcomeStatus,
    pub output: String,
    pub metadata: Metadata,
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
