use agent_core::{
    Event, EventId, EventKind, Message, MessageRole, Metadata, ModelRole, PermissionDecision,
    PermissionRequest, PermissionRequestId, PermissionResolution, PermissionRisk, TaskId,
    ToolArtifact, ToolContent, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk,
};
use agent_graph::{extract_graph_from_chunk, graph_rag_walk, FileGraphStore, GraphRagTrace, GraphStore};
use agent_memory::{
    build_restore_context_pack, build_session_checkpoint_at, CheckpointOptions, RestoreContextPack,
    SessionCheckpoint,
};
use agent_mcp::{McpCatalogService, McpServerConfig, McpTransportConfig};
use agent_rag::{
    build_grounded_answer_prompt, export_lancedb_records_jsonl, index_workspace,
    index_workspace_with_embedder, EmbeddingBatch, FileRagAdapter, IndexOptions, RagAdapter,
    RagChunk, RagEmbedder, RagIndexStats, RagSearchResult,
};
use agent_skills::{SkillCatalog, SkillPreference, SkillRecord};
use agent_runtime::{
    advance_with_model_response, append_tool_observation,
    model_request_for_turn_with_system_prompt, observation_from_tool_result,
    record_tool_outcome, repeated_tool_failure_count,
    resume_agent_loop_from_messages, start_agent_loop, start_agent_loop_with_history,
    tool_invocation_from_request, AgentAdvance, AgentRuntimeConfig, DEFAULT_AGENT_SYSTEM_PROMPT,
    MAX_IDENTICAL_TOOL_FAILURES,
};
use agent_storage::{EventStore, PermissionAuditRecord, PermissionStore, SqliteStore, StorageError};
use base64::Engine;
use model_provider::{
    EmbeddingRequest, ModelCallMode, ModelRequest, OpenAiCompatibleConfig,
    OpenAiCompatibleProvider, ModelProvider,
};
use orchestrator::{
    default_plan, parse_policy, role_label, step_prompt, LearnedModelRouter, ModelCandidate,
    OrchestrationPolicy, RoutingContext, RoutingDecision,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
use tools::ToolRegistry;

const PHASE3_TASK_ID: &str = "phase-3-demo";
const PHASE4_TASK_ID: &str = "phase-4-demo";
const PHASE5_TASK_ID: &str = "phase-5-tools";
const PHASE6_TASK_ID: &str = "phase-6-orchestration";
const PHASE7_TASK_ID: &str = "phase-7-rag";
const PHASE8_TASK_ID: &str = "phase-8-browser";
const PHASE15_TASK_ID: &str = "phase-15-context";
const PHASE16_TASK_ID: &str = "phase-16-agent-loop";

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct AppState {
    store: Mutex<SqliteStore>,
    provider_config: Mutex<ProviderConfig>,
    workspace_config: Mutex<WorkspaceConfig>,
    sidecar_config: Mutex<SidecarConfig>,
    project_session_config: Mutex<ProjectSessionConfig>,
    mcp_catalog: Mutex<McpCatalogService>,
    suspended_agent_runs: Mutex<BTreeMap<String, SuspendedAgentRun>>,
    allow_exit: AtomicBool,
    quit_prompt_active: AtomicBool,
}

#[derive(Debug, Clone)]
struct SuspendedAgentRun {
    runtime: agent_runtime::AgentLoopState,
    prompt: String,
    run_context: Metadata,
    workspace_root: PathBuf,
    collaboration: Option<AgentCollaboration>,
}

#[derive(Debug, Clone)]
struct ResolvedToolObservation {
    call_id: agent_core::ToolCallId,
    tool_name: String,
    input_json: String,
    status: ToolOutcomeStatus,
    observation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpServerView {
    id: String,
    name: String,
    enabled: bool,
    require_approval: bool,
    timeout_ms: u64,
    transport_type: String,
    command: Option<String>,
    args: Vec<String>,
    url: Option<String>,
    secret_keys: Vec<String>,
    tool_count: usize,
    refreshed_at_ms: Option<u64>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpStateView {
    servers: Vec<McpServerView>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServersInput {
    servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServerInput {
    server: McpServerConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServerPolicyInput {
    server_id: String,
    enabled: bool,
    require_approval: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillStateView {
    skills: Vec<SkillRecord>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillPreferenceInput {
    skill_id: String,
    enabled: bool,
    trusted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderConfig {
    base_url: String,
    api_key: String,
    model: String,
    planner_model: String,
    executor_model: String,
    reviewer_model: String,
    summarizer_model: String,
    embedding_model: String,
    collaboration_policy: String,
    context_window_tokens: u64,
    agent_system_prompt: String,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        let model = "gpt-4.1-mini".to_string();
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: model.clone(),
            planner_model: model.clone(),
            executor_model: model.clone(),
            reviewer_model: model.clone(),
            summarizer_model: model,
            embedding_model: "text-embedding-3-small".to_string(),
            collaboration_policy: "auto_router".to_string(),
            context_window_tokens: 128_000,
            agent_system_prompt: DEFAULT_AGENT_SYSTEM_PROMPT.to_string(),
        }
    }
}

impl ProviderConfig {
    fn is_ready(&self) -> bool {
        !self.base_url.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.executor_model.trim().is_empty()
    }

    fn model_for_role(&self, role: &ModelRole) -> String {
        let model = match role {
            ModelRole::Planner => &self.planner_model,
            ModelRole::Executor => &self.executor_model,
            ModelRole::Reviewer => &self.reviewer_model,
            ModelRole::Summarizer => &self.summarizer_model,
            ModelRole::Embedder => &self.embedding_model,
        };

        if model.trim().is_empty() {
            self.model.clone()
        } else {
            model.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceConfig {
    root: PathBuf,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            root: workspace_root(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarConfig {
    browser_path: String,
    computer_path: String,
    auto_configure: bool,
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self {
            browser_path: default_browser_sidecar_path().display().to_string(),
            computer_path: default_computer_sidecar_path().display().to_string(),
            auto_configure: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectSessionConfig {
    active_project_id: String,
    active_session_id: String,
    projects: Vec<ProjectRecord>,
    sessions: Vec<SessionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectRecord {
    id: String,
    name: String,
    root: String,
    detail: String,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRecord {
    id: String,
    project_id: String,
    name: String,
    detail: String,
    created_at_ms: u64,
    updated_at_ms: u64,
    archived_at_ms: Option<u64>,
}

impl ProjectSessionConfig {
    fn default_for_root(root: &Path) -> Self {
        let now = current_time_millis();
        let project_id = "project-cindx".to_string();
        let session_id = "session-runtime".to_string();
        Self {
            active_project_id: project_id.clone(),
            active_session_id: session_id.clone(),
            projects: vec![ProjectRecord {
                id: project_id.clone(),
                name: "Cindx".to_string(),
                root: root.display().to_string(),
                detail: "current workspace".to_string(),
                created_at_ms: now,
                updated_at_ms: now,
            }],
            sessions: vec![SessionRecord {
                id: session_id,
                project_id,
                name: "Runtime Session".to_string(),
                detail: "timeline + chat".to_string(),
                created_at_ms: now,
                updated_at_ms: now,
                archived_at_ms: None,
            }],
        }
    }

    fn active_project(&self) -> Option<&ProjectRecord> {
        self.projects
            .iter()
            .find(|project| project.id == self.active_project_id)
    }

    fn active_session(&self) -> Option<&SessionRecord> {
        self.sessions
            .iter()
            .find(|session| session.id == self.active_session_id)
    }

    fn ensure_consistent(&mut self, fallback_root: &Path) {
        if self.projects.is_empty() {
            *self = Self::default_for_root(fallback_root);
            return;
        }
        if !self.projects.iter().any(|project| project.id == self.active_project_id) {
            self.active_project_id = self.projects[0].id.clone();
        }
        if !self
            .sessions
            .iter()
            .any(|session| {
                session.project_id == self.active_project_id && session.archived_at_ms.is_none()
            })
        {
            let now = current_time_millis();
            let session_id = unique_config_id(
                "session",
                "Runtime Session",
                &self
                    .sessions
                    .iter()
                    .map(|session| session.id.clone())
                    .collect::<Vec<_>>(),
            );
            self.sessions.push(SessionRecord {
                id: session_id,
                project_id: self.active_project_id.clone(),
                name: "Runtime Session".to_string(),
                detail: "timeline + chat".to_string(),
                created_at_ms: now,
                updated_at_ms: now,
                archived_at_ms: None,
            });
        }
        if !self
            .sessions
            .iter()
            .any(|session| {
                session.id == self.active_session_id && session.archived_at_ms.is_none()
            })
        {
            self.active_session_id = self
                .sessions
                .iter()
                .find(|session| {
                    session.project_id == self.active_project_id
                        && session.archived_at_ms.is_none()
                })
                .or_else(|| {
                    self.sessions
                        .iter()
                        .find(|session| session.archived_at_ms.is_none())
                })
                .map(|session| session.id.clone())
                .unwrap_or_default();
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    app_version: String,
    kernel_status: String,
    workspace_root: String,
    orchestration_modes: Vec<String>,
    registered_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SidecarState {
    browser: SidecarEndpointState,
    computer: SidecarEndpointState,
    auto_configure: bool,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SidecarEndpointState {
    path: String,
    exists: bool,
    executable: bool,
    healthy: bool,
    health_output: String,
    env_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSessionState {
    projects: Vec<ProjectView>,
    sessions: Vec<SessionView>,
    active_project_id: String,
    active_session_id: String,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectView {
    id: String,
    name: String,
    root: String,
    detail: String,
    status: String,
    active: bool,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionView {
    id: String,
    project_id: String,
    name: String,
    detail: String,
    status: String,
    active: bool,
    archived: bool,
    archived_at_ms: Option<u64>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceInput {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectInput {
    name: String,
    root: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionInput {
    name: String,
    project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameSessionInput {
    session_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectProjectInput {
    project_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectSessionInput {
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionActionInput {
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarConfigInput {
    browser_path: String,
    computer_path: String,
    auto_configure: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineEntry {
    label: String,
    detail: String,
    kind: String,
    state: String,
    timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionAudit {
    id: String,
    risk: String,
    action: String,
    reason: String,
    scope: String,
    status: String,
    decision: Option<String>,
    requested_at_ms: u64,
    resolved_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Phase3State {
    timeline: Vec<TimelineEntry>,
    permissions: Vec<PermissionAudit>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderConfigState {
    base_url: String,
    model: String,
    planner_model: String,
    executor_model: String,
    reviewer_model: String,
    summarizer_model: String,
    embedding_model: String,
    collaboration_policy: String,
    context_window_tokens: u64,
    agent_system_prompt: String,
    api_key_set: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatMessageView {
    role: String,
    content: String,
    timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Phase4State {
    provider: ProviderConfigState,
    timeline: Vec<TimelineEntry>,
    messages: Vec<ChatMessageView>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolSpecView {
    name: String,
    description: String,
    risk: String,
    input_schema: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolRunView {
    invocation_id: String,
    tool_name: String,
    status: String,
    output: String,
    timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolApprovalView {
    request_id: String,
    invocation_id: String,
    tool_name: String,
    risk: String,
    reason: String,
    scope: String,
    input: String,
    requested_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Phase5State {
    timeline: Vec<TimelineEntry>,
    tools: Vec<ToolSpecView>,
    pending_approvals: Vec<ToolApprovalView>,
    results: Vec<ToolRunView>,
    last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolRunInput {
    tool_name: String,
    input: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrchestrationStepView {
    orchestration_id: String,
    policy: String,
    step_index: u64,
    role: String,
    model: String,
    output: String,
    latency_ms: Option<u64>,
    timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Phase6State {
    timeline: Vec<TimelineEntry>,
    steps: Vec<OrchestrationStepView>,
    last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrchestrationRunInput {
    policy: String,
    prompt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RagStatsView {
    files_indexed: usize,
    chunks_indexed: usize,
    indexed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RagSourceView {
    path: String,
    start_line: u64,
    end_line: u64,
    file_hash: String,
    score: f32,
    reason: String,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Phase7State {
    timeline: Vec<TimelineEntry>,
    stats: RagStatsView,
    sources: Vec<RagSourceView>,
    answer: Option<String>,
    last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RagSearchInput {
    query: String,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserObservationView {
    invocation_id: String,
    tool_name: String,
    status: String,
    url: Option<String>,
    output: String,
    artifact_path: Option<String>,
    text_path: Option<String>,
    capture_kind: Option<String>,
    timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Phase8State {
    timeline: Vec<TimelineEntry>,
    pending_approvals: Vec<ToolApprovalView>,
    observations: Vec<BrowserObservationView>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextCheckpointView {
    id: String,
    generated_at_ms: u64,
    event_count: usize,
    task_count: usize,
    latest_event_ms: u64,
    current_goal: Option<String>,
    completed_steps: Vec<String>,
    pending_steps: Vec<String>,
    decisions: Vec<String>,
    file_changes: Vec<String>,
    commands_run: Vec<String>,
    tool_results: Vec<String>,
    retrievals: Vec<String>,
    artifacts: Vec<String>,
    errors: Vec<String>,
    next_actions: Vec<String>,
    path: Option<String>,
    restore_pack: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextState {
    timeline: Vec<TimelineEntry>,
    checkpoint: Option<ContextCheckpointView>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentState {
    task_id: String,
    project_id: Option<String>,
    project_name: Option<String>,
    session_id: Option<String>,
    session_name: Option<String>,
    status: String,
    turn_count: usize,
    max_turns: usize,
    transcript_messages: usize,
    context_tokens_used: u64,
    context_window_tokens: u64,
    context_remaining_percent: f64,
    context_usage_estimated: bool,
    can_cancel: bool,
    can_retry: bool,
    timeline: Vec<TimelineEntry>,
    messages: Vec<ChatMessageView>,
    pending_approvals: Vec<ToolApprovalView>,
    latest_answer: Option<String>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTraceState {
    task_id: String,
    trace_id: String,
    run_id: String,
    project_id: Option<String>,
    project_name: Option<String>,
    session_id: Option<String>,
    session_name: Option<String>,
    status: String,
    started_at_ms: u64,
    finished_at_ms: Option<u64>,
    duration_ms: Option<u64>,
    turn_count: usize,
    step_count: usize,
    tool_call_count: usize,
    permission_wait_count: usize,
    error_count: usize,
    export_path: Option<String>,
    turns: Vec<AgentTraceTurnView>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTraceTurnView {
    index: usize,
    label: String,
    status: String,
    started_at_ms: u64,
    finished_at_ms: Option<u64>,
    duration_ms: Option<u64>,
    steps: Vec<AgentTraceStepView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTraceStepView {
    id: String,
    parent_id: Option<String>,
    turn_index: usize,
    sequence: u64,
    kind: String,
    label: String,
    status: String,
    started_at_ms: u64,
    finished_at_ms: Option<u64>,
    latency_ms: Option<u64>,
    model: Option<String>,
    tool_name: Option<String>,
    request_id: Option<String>,
    tool_call_id: Option<String>,
    permission_id: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    artifact_path: Option<String>,
    detail: String,
    metadata: Metadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskInput {
    prompt: String,
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserToolInput {
    tool_name: String,
    input: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderConfigInput {
    base_url: String,
    api_key: String,
    model: String,
    planner_model: String,
    executor_model: String,
    reviewer_model: String,
    summarizer_model: String,
    embedding_model: String,
    collaboration_policy: String,
    context_window_tokens: u64,
    agent_system_prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderModelsInput {
    base_url: String,
    api_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderModelsState {
    models: Vec<String>,
    fetched_at_ms: u64,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelStreamDelta {
    task_id: String,
    request_id: String,
    delta: String,
    done: bool,
    error: Option<String>,
}

#[tauri::command]
fn get_runtime_status(state: tauri::State<'_, AppState>) -> Result<RuntimeStatus, String> {
    runtime_status(&state)
}

#[tauri::command]
fn get_sidecar_state(state: tauri::State<'_, AppState>) -> Result<SidecarState, String> {
    let config = state
        .sidecar_config
        .lock()
        .map_err(|error| format!("sidecar config lock poisoned: {error}"))?
        .clone();
    Ok(sidecar_state(&config, None))
}

#[tauri::command]
fn save_sidecar_config(
    state: tauri::State<'_, AppState>,
    input: SidecarConfigInput,
) -> Result<SidecarState, String> {
    let config = SidecarConfig {
        browser_path: normalized_config_value(&input.browser_path),
        computer_path: normalized_config_value(&input.computer_path),
        auto_configure: input.auto_configure,
    };
    save_sidecar_config_to_disk(&config).map_err(|error| error.to_string())?;
    apply_sidecar_env(&config);
    let mut stored = state
        .sidecar_config
        .lock()
        .map_err(|error| format!("sidecar config lock poisoned: {error}"))?;
    *stored = config.clone();
    Ok(sidecar_state(&config, None))
}

#[tauri::command]
fn get_mcp_state(state: tauri::State<'_, AppState>) -> Result<McpStateView, String> {
    let catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
fn save_mcp_servers(
    state: tauri::State<'_, AppState>,
    input: McpServersInput,
) -> Result<McpStateView, String> {
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    catalog
        .save_servers(input.servers)
        .map_err(|error| error.to_string())?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
fn upsert_mcp_server(
    state: tauri::State<'_, AppState>,
    input: McpServerInput,
) -> Result<McpStateView, String> {
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    catalog
        .upsert_server(input.server)
        .map_err(|error| error.to_string())?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
fn update_mcp_server_policy(
    state: tauri::State<'_, AppState>,
    input: McpServerPolicyInput,
) -> Result<McpStateView, String> {
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    catalog
        .update_policy(&input.server_id, input.enabled, input.require_approval)
        .map_err(|error| error.to_string())?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
fn remove_mcp_server(
    state: tauri::State<'_, AppState>,
    server_id: String,
) -> Result<McpStateView, String> {
    let mut catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    catalog
        .remove_server(&server_id)
        .map_err(|error| error.to_string())?;
    Ok(mcp_state_view(&catalog, None))
}

#[tauri::command]
async fn refresh_mcp_server(
    app: tauri::AppHandle,
    server_id: String,
) -> Result<McpStateView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut catalog = state
            .mcp_catalog
            .lock()
            .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
        let last_error = catalog
            .refresh_server(&server_id)
            .err()
            .map(|error| error.to_string());
        Ok(mcp_state_view(&catalog, last_error))
    })
    .await
    .map_err(|error| format!("MCP refresh failed to join: {error}"))?
}

fn mcp_state_view(catalog: &McpCatalogService, last_error: Option<String>) -> McpStateView {
    let servers = catalog
        .states()
        .into_iter()
        .map(|state| {
            let (transport_type, command, args, url, secret_keys) = match &state.config.transport {
                McpTransportConfig::Stdio { command, args, env } => (
                    "stdio".to_string(),
                    Some(command.clone()),
                    args.clone(),
                    None,
                    env.keys().cloned().collect(),
                ),
                McpTransportConfig::StreamableHttp { url, headers } => (
                    "streamable_http".to_string(),
                    None,
                    Vec::new(),
                    Some(url.clone()),
                    headers.keys().cloned().collect(),
                ),
            };
            McpServerView {
                id: state.config.id,
                name: state.config.name,
                enabled: state.config.enabled,
                require_approval: state.config.require_approval,
                timeout_ms: state.config.timeout_ms,
                transport_type,
                command,
                args,
                url,
                secret_keys,
                tool_count: state.tool_count,
                refreshed_at_ms: state.refreshed_at_ms,
                last_error: state.last_error,
            }
        })
        .collect();
    McpStateView { servers, last_error }
}

#[tauri::command]
fn get_skill_state(state: tauri::State<'_, AppState>) -> Result<SkillStateView, String> {
    let root = active_workspace_root(&state)?;
    let catalog = skill_catalog_for_root(&root);
    Ok(SkillStateView {
        skills: catalog.list(),
        last_error: None,
    })
}

#[tauri::command]
fn refresh_skills(state: tauri::State<'_, AppState>) -> Result<SkillStateView, String> {
    let root = active_workspace_root(&state)?;
    let catalog = skill_catalog_for_root(&root);
    let skills = catalog.refresh()?;
    Ok(SkillStateView {
        skills,
        last_error: None,
    })
}

#[tauri::command]
fn save_skill_preference(
    state: tauri::State<'_, AppState>,
    input: SkillPreferenceInput,
) -> Result<SkillStateView, String> {
    let root = active_workspace_root(&state)?;
    let catalog = skill_catalog_for_root(&root);
    let skills = catalog.set_preference(
        &input.skill_id,
        SkillPreference {
            enabled: input.enabled,
            trusted: input.trusted,
        },
    )?;
    Ok(SkillStateView {
        skills,
        last_error: None,
    })
}

#[tauri::command]
fn get_project_session_state(
    state: tauri::State<'_, AppState>,
) -> Result<ProjectSessionState, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn create_project(
    state: tauri::State<'_, AppState>,
    input: CreateProjectInput,
) -> Result<ProjectSessionState, String> {
    let name = normalized_config_value(&input.name);
    if name.is_empty() {
        return project_session_state_with_error(&state, "project name is empty");
    }
    let root = validate_workspace_root(&input.root)?;
    let mut workspace_config = state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let project_id = unique_config_id(
        "project",
        &name,
        &config
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect::<Vec<_>>(),
    );
    let session_id = unique_config_id(
        "session",
        "New Session",
        &config
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>(),
    );
    let now = current_time_millis();
    config.projects.push(ProjectRecord {
        id: project_id.clone(),
        name: name.clone(),
        root: root.display().to_string(),
        detail: "workspace project".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
    });
    config.sessions.push(SessionRecord {
        id: session_id.clone(),
        project_id: project_id.clone(),
        name: "New Session".to_string(),
        detail: "timeline + chat".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        archived_at_ms: None,
    });
    config.active_project_id = project_id;
    config.active_session_id = session_id;
    workspace_config.root = root;
    save_workspace_config_to_disk(&workspace_config).map_err(|error| error.to_string())?;
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;

    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn create_session(
    state: tauri::State<'_, AppState>,
    input: CreateSessionInput,
) -> Result<ProjectSessionState, String> {
    let name = normalized_config_value(&input.name);
    if name.is_empty() {
        return project_session_state_with_error(&state, "session name is empty");
    }
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let project_id = input
        .project_id
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| config.active_project_id.clone());
    if !config.projects.iter().any(|project| project.id == project_id) {
        return Ok(project_session_state(
            &config,
            Some("project not found for session".to_string()),
        ));
    }
    let session_id = unique_config_id(
        "session",
        &name,
        &config
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>(),
    );
    let now = current_time_millis();
    config.sessions.push(SessionRecord {
        id: session_id.clone(),
        project_id: project_id.clone(),
        name,
        detail: "timeline + chat".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        archived_at_ms: None,
    });
    config.active_project_id = project_id;
    config.active_session_id = session_id;
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;

    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn rename_session(
    state: tauri::State<'_, AppState>,
    input: RenameSessionInput,
) -> Result<ProjectSessionState, String> {
    let name = normalized_config_value(&input.name);
    if name.is_empty() {
        return project_session_state_with_error(&state, "session name is empty");
    }
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let now = current_time_millis();
    let project_id = {
        let Some(session) = config
            .sessions
            .iter_mut()
            .find(|session| session.id == input.session_id)
        else {
            return Ok(project_session_state(
                &config,
                Some("session not found".to_string()),
            ));
        };
        session.name = name;
        session.updated_at_ms = now;
        session.project_id.clone()
    };
    if let Some(project) = config
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
    {
        project.updated_at_ms = now;
    }
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn fork_session(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let (source, project, fork) = {
        let mut config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        let Some(source) = config
            .sessions
            .iter()
            .find(|session| session.id == input.session_id)
            .cloned()
        else {
            return Ok(project_session_state(
                &config,
                Some("session not found".to_string()),
            ));
        };
        let Some(project) = config
            .projects
            .iter()
            .find(|project| project.id == source.project_id)
            .cloned()
        else {
            return Ok(project_session_state(
                &config,
                Some("session project not found".to_string()),
            ));
        };
        let name = unique_fork_name(&config, &source);
        let id = unique_config_id(
            "session",
            &name,
            &config
                .sessions
                .iter()
                .map(|session| session.id.clone())
                .collect::<Vec<_>>(),
        );
        let now = current_time_millis();
        let fork = SessionRecord {
            id: id.clone(),
            project_id: source.project_id.clone(),
            name,
            detail: format!("Fork of {}", source.name),
            created_at_ms: now,
            updated_at_ms: now,
            archived_at_ms: None,
        };
        config.sessions.push(fork.clone());
        config.active_project_id = source.project_id.clone();
        config.active_session_id = id;
        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
        (source, project, fork)
    };

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let events = store
        .list_by_task(&phase16_task_id())
        .map_err(|error| error.to_string())?;
    for event in agent_session_events(&events, &source.id) {
        let mut metadata = event.metadata;
        metadata.insert("session_id".to_string(), fork.id.clone());
        metadata.insert("session_name".to_string(), fork.name.clone());
        metadata.insert("project_id".to_string(), project.id.clone());
        metadata.insert("project_name".to_string(), project.name.clone());
        metadata.insert("project_root".to_string(), project.root.clone());
        metadata.insert("forked_from_session_id".to_string(), source.id.clone());
        append_event(
            &mut store,
            &phase16_task_id(),
            event.kind,
            event.summary,
            metadata,
        )
        .map_err(|error| error.to_string())?;
    }

    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn archive_session(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let Some(index) = config
        .sessions
        .iter()
        .position(|session| session.id == input.session_id)
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    let project_id = config.sessions[index].project_id.clone();
    let now = current_time_millis();
    config.sessions[index].archived_at_ms = Some(now);
    config.sessions[index].updated_at_ms = now;
    if config.active_session_id == input.session_id {
        config.active_session_id = ensure_open_session_for_project(&mut config, &project_id);
    }
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn restore_session(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let Some(session) = config
        .sessions
        .iter_mut()
        .find(|session| session.id == input.session_id)
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    session.archived_at_ms = None;
    session.updated_at_ms = current_time_millis();
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn delete_session(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let deleted_event_ids = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = store
            .list_by_task(&phase16_task_id())
            .map_err(|error| error.to_string())?;
        agent_session_events(&events, &input.session_id)
            .into_iter()
            .map(|event| event.id.0)
            .collect::<Vec<_>>()
    };

    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let Some(index) = config
        .sessions
        .iter()
        .position(|session| session.id == input.session_id)
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    let project_id = config.sessions[index].project_id.clone();
    config.sessions.remove(index);
    if config.active_session_id == input.session_id {
        config.active_session_id = ensure_open_session_for_project(&mut config, &project_id);
    }
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    let next_state = project_session_state(&config, None);
    drop(config);

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    store
        .delete_events_by_ids(&deleted_event_ids)
        .map_err(|error| error.to_string())?;
    store
        .delete_records_by_metadata("session_id", &input.session_id)
        .map_err(|error| error.to_string())?;
    Ok(next_state)
}

#[tauri::command]
fn select_project(
    state: tauri::State<'_, AppState>,
    input: SelectProjectInput,
) -> Result<ProjectSessionState, String> {
    let mut workspace_config = state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let Some(project) = config
        .projects
        .iter()
        .find(|project| project.id == input.project_id)
        .cloned()
    else {
        return Ok(project_session_state(&config, Some("project not found".to_string())));
    };
    let root = validate_workspace_root(&project.root)?;
    config.active_project_id = project.id.clone();
    if !config
        .sessions
        .iter()
        .any(|session| {
            session.id == config.active_session_id
                && session.project_id == project.id
                && session.archived_at_ms.is_none()
        })
    {
        config.active_session_id = config
            .sessions
            .iter()
            .find(|session| {
                session.project_id == project.id && session.archived_at_ms.is_none()
            })
            .map(|session| session.id.clone())
            .unwrap_or_else(|| {
                let session_id = unique_config_id(
                    "session",
                    &format!("{} Session", project.name),
                    &config
                        .sessions
                        .iter()
                        .map(|session| session.id.clone())
                        .collect::<Vec<_>>(),
                );
                let now = current_time_millis();
                config.sessions.push(SessionRecord {
                    id: session_id.clone(),
                    project_id: project.id.clone(),
                    name: format!("{} Session", project.name),
                    detail: "timeline + chat".to_string(),
                    created_at_ms: now,
                    updated_at_ms: now,
                    archived_at_ms: None,
                });
                session_id
            });
    }
    workspace_config.root = root;
    save_workspace_config_to_disk(&workspace_config).map_err(|error| error.to_string())?;
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;

    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn select_session(
    state: tauri::State<'_, AppState>,
    input: SelectSessionInput,
) -> Result<ProjectSessionState, String> {
    let mut workspace_config = state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let Some(session) = config
        .sessions
        .iter()
        .find(|session| session.id == input.session_id && session.archived_at_ms.is_none())
        .cloned()
    else {
        return Ok(project_session_state(&config, Some("session not found".to_string())));
    };
    let Some(project) = config
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .cloned()
    else {
        return Ok(project_session_state(
            &config,
            Some("session project not found".to_string()),
        ));
    };
    let root = validate_workspace_root(&project.root)?;
    config.active_project_id = project.id;
    config.active_session_id = session.id;
    workspace_config.root = root;
    save_workspace_config_to_disk(&workspace_config).map_err(|error| error.to_string())?;
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;

    Ok(project_session_state(&config, None))
}

#[tauri::command]
fn save_workspace_root(
    state: tauri::State<'_, AppState>,
    input: WorkspaceInput,
) -> Result<RuntimeStatus, String> {
    let root = validate_workspace_root(&input.path)?;
    {
        let mut config = state
            .workspace_config
            .lock()
            .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
        config.root = root.clone();
        save_workspace_config_to_disk(&config).map_err(|error| error.to_string())?;
    }
    sync_active_project_root(&state, &root)?;

    runtime_status(&state)
}

fn runtime_status(state: &tauri::State<'_, AppState>) -> Result<RuntimeStatus, String> {
    let root = active_workspace_root(state)?;
    let mut status = runtime_status_for_root(root.clone());
    let registry = tool_registry_for_state(state, &root)?;
    status.registered_tools = registry
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .chain(["rag.index", "rag.search", "rag.answer"].into_iter().map(str::to_string))
        .collect();
    status.registered_tools.sort();
    status.registered_tools.dedup();
    Ok(status)
}

fn runtime_status_for_root(root: PathBuf) -> RuntimeStatus {
    RuntimeStatus {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        kernel_status: "kernel bridge online".to_string(),
        workspace_root: root.display().to_string(),
        orchestration_modes: vec![
            OrchestrationPolicy::Single.label().to_string(),
            OrchestrationPolicy::PlanExecuteReview.label().to_string(),
            OrchestrationPolicy::BestOfN { candidates: 3 }
                .label()
                .to_string(),
            OrchestrationPolicy::AutoRouter.label().to_string(),
        ],
        registered_tools: vec![
            "file.read".to_string(),
            "file.list".to_string(),
            "file.write".to_string(),
            "file.search".to_string(),
            "shell.run".to_string(),
            "rag.index".to_string(),
            "rag.search".to_string(),
            "rag.answer".to_string(),
            "web.search".to_string(),
            "browser.open".to_string(),
            "browser.extract_text".to_string(),
            "browser.capture".to_string(),
            "browser.click".to_string(),
            "browser.type".to_string(),
            "browser.scroll".to_string(),
            "computer.screenshot".to_string(),
            "computer.click".to_string(),
            "computer.type".to_string(),
            "computer.key".to_string(),
            "computer.scroll".to_string(),
        ],
    }
}

#[tauri::command]
fn get_phase3_state(state: tauri::State<'_, AppState>) -> Result<Phase3State, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase3_state(&store).map_err(|error| error.to_string())
}

#[tauri::command]
fn request_mock_permission(state: tauri::State<'_, AppState>) -> Result<Phase3State, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    request_mock_permission_in_store(&mut store).map_err(|error| error.to_string())
}

#[tauri::command]
fn resolve_permission(
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
) -> Result<Phase3State, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    resolve_permission_in_store(&mut store, &request_id, &decision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_phase4_state(state: tauri::State<'_, AppState>) -> Result<Phase4State, String> {
    let config = clone_provider_config(&state)?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase4_state(&store, &config, None).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_provider_config(
    state: tauri::State<'_, AppState>,
    input: ProviderConfigInput,
) -> Result<Phase4State, String> {
    let config = {
        let mut config = state
            .provider_config
            .lock()
            .map_err(|error| format!("provider config lock poisoned: {error}"))?;
        apply_provider_config_input(&mut config, input);
        save_provider_config_to_disk(&config).map_err(|error| error.to_string())?;
        config.clone()
    };

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase4_task_id(),
        EventKind::TaskStatusChanged,
        "Provider config saved",
        [
            ("provider".to_string(), "openai-compatible".to_string()),
            ("base_url".to_string(), config.base_url.clone()),
            (
                "executor_model".to_string(),
                config.model_for_role(&ModelRole::Executor),
            ),
            (
                "agent_system_prompt_length".to_string(),
                config.agent_system_prompt.chars().count().to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase4_state(&store, &config, None).map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_provider_models(
    app: tauri::AppHandle,
    input: ProviderModelsInput,
) -> Result<ProviderModelsState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut config = clone_provider_config(&state)?;
        let base_url = normalized_config_value(&input.base_url);
        let api_key = normalized_config_value(&input.api_key);
        if !base_url.is_empty() {
            config.base_url = base_url;
        }
        if !api_key.is_empty() {
            config.api_key = api_key;
        }

        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url,
            api_key: config.api_key,
            model: config.model,
            embedding_model: config.embedding_model,
            timeout_seconds: 30,
        });
        let models = provider.list_models().map_err(|error| error.to_string())?;
        Ok(ProviderModelsState {
            models,
            fetched_at_ms: current_time_millis(),
            last_error: None,
        })
    })
    .await
    .map_err(|error| format!("model list task failed: {error}"))?
}

#[tauri::command]
fn send_model_prompt(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    prompt: String,
) -> Result<Phase4State, String> {
    let prompt = prompt.trim().to_string();
    let config = clone_provider_config(&state)?;
    let task_id = phase4_task_id();

    if prompt.is_empty() {
        return phase4_state_with_error(&state, &config, "prompt is empty");
    }

    if !config.is_ready() {
        record_phase4_error(&state, "Provider config is incomplete")?;
        return phase4_state_with_error(&state, &config, "Provider config is incomplete");
    }

    let request_id = unique_id("model");
    let model = config.model_for_role(&ModelRole::Executor);
    let started_at_ms = current_time_millis();

    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_message_event(&mut store, &task_id, MessageRole::User, &prompt)
            .map_err(|error| error.to_string())?;
        append_event(
            &mut store,
            &task_id,
            EventKind::ModelRequestStarted,
            format!("Model request started for {model}"),
            [
                ("request_id".to_string(), request_id.clone()),
                ("provider".to_string(), "openai-compatible".to_string()),
                ("base_url".to_string(), config.base_url.clone()),
                ("model".to_string(), model.clone()),
                ("role".to_string(), "executor".to_string()),
                ("prompt_length".to_string(), prompt.len().to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: 180,
    });
    let mut request_metadata = Metadata::new();
    request_metadata.insert("request_id".to_string(), request_id.clone());
    let request = ModelRequest {
        role: ModelRole::Executor,
        messages: vec![Message {
            role: MessageRole::User,
            content: prompt,
            metadata: Metadata::new(),
        }],
        tools: Vec::new(),
        mode: ModelCallMode::Streaming,
        metadata: request_metadata,
    };
    let stream_task_id = task_id.0.clone();
    let stream_request_id = request_id.clone();
    let stream_app = app.clone();
    let result = provider.complete_streaming(request, |delta| {
        let _ = stream_app.emit(
            "model-stream-delta",
            ModelStreamDelta {
                task_id: stream_task_id.clone(),
                request_id: stream_request_id.clone(),
                delta: delta.to_string(),
                done: false,
                error: None,
            },
        );
    });

    match result {
        Ok(response) => {
            let latency_ms = current_time_millis().saturating_sub(started_at_ms);
            let _ = app.emit(
                "model-stream-delta",
                ModelStreamDelta {
                    task_id: task_id.0.clone(),
                    request_id: request_id.clone(),
                    delta: String::new(),
                    done: true,
                    error: None,
                },
            );

            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_event(
                &mut store,
                &task_id,
                EventKind::ModelRequestFinished,
                format!("Model response received from {model}"),
                [
                    ("request_id".to_string(), request_id),
                    ("provider".to_string(), "openai-compatible".to_string()),
                    ("model".to_string(), model),
                    ("latency_ms".to_string(), latency_ms.to_string()),
                    (
                        "output_length".to_string(),
                        response.message.content.len().to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            )
            .map_err(|error| error.to_string())?;
            append_message_event(
                &mut store,
                &task_id,
                MessageRole::Assistant,
                &response.message.content,
            )
            .map_err(|error| error.to_string())?;

            phase4_state(&store, &config, None).map_err(|error| error.to_string())
        }
        Err(error) => {
            let message = error.to_string();
            let _ = app.emit(
                "model-stream-delta",
                ModelStreamDelta {
                    task_id: task_id.0.clone(),
                    request_id,
                    delta: String::new(),
                    done: true,
                    error: Some(message.clone()),
                },
            );
            record_phase4_error(&state, &message)?;
            phase4_state_with_error(&state, &config, &message)
        }
    }
}

#[tauri::command]
fn get_agent_state(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
) -> Result<AgentState, String> {
    let run_context = project_session_metadata_for_session(&state, session_id.as_deref())?;
    let session_id = run_context.get("session_id").map(String::as_str);
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_agent_trace_state(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
) -> Result<AgentTraceState, String> {
    let run_context = project_session_metadata_for_session(&state, session_id.as_deref())?;
    let session_id = run_context.get("session_id").map(String::as_str);
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    agent_trace_state_for_session(&store, None, None, session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn export_agent_trace_jsonl(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
) -> Result<AgentTraceState, String> {
    let run_context = project_session_metadata_for_session(&state, session_id.as_deref())?;
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let session_id = run_context.get("session_id").map(String::as_str);
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let export_path = write_agent_trace_jsonl(&root, &store, session_id)
        .map_err(|error| error.to_string())?;
    agent_trace_state_for_session(&store, Some(export_path), None, session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn run_agent_task(
    app: tauri::AppHandle,
    input: AgentTaskInput,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        run_agent_task_blocking(state, input)
    })
    .await
    .map_err(|error| format!("agent task failed to join: {error}"))?
}

fn maybe_auto_name_session(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
    prompt: &str,
) -> Result<(), String> {
    let should_rename = {
        let config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        config
            .sessions
            .iter()
            .find(|session| session.id == session_id && session.archived_at_ms.is_none())
            .is_some_and(|session| is_automatic_session_name(&session.name))
    };
    if !should_rename {
        return Ok(());
    }

    let title = automatic_session_title(prompt);
    let now = current_time_millis();
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let project_id = {
        let Some(session) = config
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id && is_automatic_session_name(&session.name))
        else {
            return Ok(());
        };
        session.name = title;
        session.updated_at_ms = now;
        session.project_id.clone()
    };
    if let Some(project) = config
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
    {
        project.updated_at_ms = now;
    }
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())
}

fn is_automatic_session_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "runtime session" | "new session" | "untitled session" | "session"
    )
}

fn automatic_session_title(prompt: &str) -> String {
    let compact = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = compact.chars().take(36).collect::<String>();
    let title = title.trim_end();
    if title.is_empty() {
        "New Session".to_string()
    } else {
        title.to_string()
    }
}

fn run_agent_task_blocking(
    state: tauri::State<'_, AppState>,
    input: AgentTaskInput,
) -> Result<AgentState, String> {
    let prompt = input.prompt.trim().to_string();
    let session_id = input.session_id;
    clear_suspended_agent_run(&state, &session_id)?;
    let mut run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    if prompt.is_empty() {
        return agent_state_with_error_in_context(&state, &run_context, "agent prompt is empty");
    }
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }

    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    maybe_auto_name_session(&state, &session_id, &prompt)?;
    run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    run_context.insert("agent_run_id".to_string(), unique_id("agent-run"));
    let requested_policy = parse_policy(&config.collaboration_policy)
        .unwrap_or(OrchestrationPolicy::AutoRouter);
    let mut routing_context =
        RoutingContext::from_prompt(&prompt, model_candidates_for_config(&config));
    if requested_policy != OrchestrationPolicy::AutoRouter {
        routing_context.user_policy_override = Some(requested_policy.clone());
    }
    let routing_decision = LearnedModelRouter::train(&[]).route(&routing_context);
    let collaboration_policy = if requested_policy == OrchestrationPolicy::AutoRouter {
        routing_decision.policy.clone()
    } else {
        requested_policy.clone()
    };
    let session_id = run_context.get("session_id").map(String::as_str);
    let task_id = phase16_task_id();
    let mut history = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = store
            .list_by_task(&task_id)
            .map_err(|error| error.to_string())?;
        let history = session_id
            .map(|session_id| agent_session_events(&events, session_id))
            .unwrap_or_default()
            .iter()
            .filter_map(message_from_event)
            .collect::<Vec<_>>();
        let mut start_metadata = run_context.clone();
        start_metadata.insert("prompt".to_string(), prompt.clone());
        start_metadata.insert(
            "requested_policy".to_string(),
            requested_policy.label().to_string(),
        );
        start_metadata.insert(
            "collaboration_policy".to_string(),
            collaboration_policy.label().to_string(),
        );
        start_metadata.insert(
            "router_explanation".to_string(),
            routing_decision.explanation.clone(),
        );
        start_metadata.insert(
            "context_window_tokens".to_string(),
            config.context_window_tokens.to_string(),
        );
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task started",
            start_metadata,
        )
        .map_err(|error| error.to_string())?;
        append_message_event_with_metadata(
            &mut store,
            &task_id,
            MessageRole::User,
            &prompt,
            run_context.clone(),
        )
            .map_err(|error| error.to_string())?;
        history
    };

    if let Some(skill_context) = skill_catalog_for_root(&root).context_for_prompt(&prompt)? {
        history.push(Message {
            role: MessageRole::System,
            content: skill_context,
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "skill_context".to_string()),
            ]
            .into_iter()
            .collect(),
        });
    }

    let collaboration = if collaboration_policy != OrchestrationPolicy::Single {
        let collaboration_id = unique_id("collab");
        let planner_prompt = build_collaboration_planner_prompt(&prompt, &history);
        let planner_output = run_collaboration_stage(
            &state,
            &config,
            &task_id,
            &run_context,
            &collaboration_id,
            "planner",
            ModelRole::Planner,
            &config.model_for_role(&ModelRole::Planner),
            planner_prompt,
        )
        .unwrap_or_default();
        if !planner_output.is_empty() {
            history.push(Message {
                role: MessageRole::System,
                content: format!("Planner guidance for the next user request:\n{planner_output}"),
                metadata: [
                    ("internal".to_string(), "true".to_string()),
                    ("collaboration_stage".to_string(), "planner".to_string()),
                ]
                .into_iter()
                .collect(),
            });
        }
        Some(AgentCollaboration {
            id: collaboration_id,
            policy: collaboration_policy.label().to_string(),
            planner_output,
        })
    } else {
        None
    };

    let runtime = if history.is_empty() {
        start_agent_loop(task_id, prompt.clone(), AgentRuntimeConfig::default())
    } else {
        start_agent_loop_with_history(
            task_id,
            prompt.clone(),
            history,
            AgentRuntimeConfig::default(),
        )
    };
    continue_agent_loop(
        &state,
        &config,
        &root,
        runtime,
        prompt,
        run_context,
        collaboration.as_ref(),
    )
}

#[tauri::command]
fn cancel_agent_task(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    clear_suspended_agent_run(&state, &input.session_id)?;
    let run_context = project_session_metadata_for_session(&state, Some(&input.session_id))?;
    let session_id = run_context.get("session_id").map(String::as_str);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let current = agent_state_for_session(&store, None, session_id)
        .map_err(|error| error.to_string())?;
    if !current.can_cancel {
        return Ok(current);
    }

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        metadata_with_context(
            [("reason".to_string(), "user_cancelled".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .map_err(|error| error.to_string())?;

    agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn retry_agent_task(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    clear_suspended_agent_run(&state, &input.session_id)?;
    let mut run_context = project_session_metadata_for_session(&state, Some(&input.session_id))?;
    run_context.insert("agent_run_id".to_string(), unique_id("agent-run"));
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let session_id = run_context.get("session_id").map(String::as_str);
    let task_id = phase16_task_id();
    let prompt = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = store
            .list_by_task(&task_id)
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, session_id);
        let prompt = latest_agent_prompt_from_active_events(&active_events)
            .ok_or_else(|| "No previous agent prompt to retry".to_string())?;
        let mut start_metadata = run_context.clone();
        start_metadata.insert("prompt".to_string(), prompt.clone());
        start_metadata.insert(
            "collaboration_policy".to_string(),
            config.collaboration_policy.clone(),
        );
        start_metadata.insert(
            "context_window_tokens".to_string(),
            config.context_window_tokens.to_string(),
        );
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task retry started",
            start_metadata,
        )
        .map_err(|error| error.to_string())?;
        append_message_event_with_metadata(
            &mut store,
            &task_id,
            MessageRole::User,
            &prompt,
            run_context.clone(),
        )
            .map_err(|error| error.to_string())?;
        prompt
    };

    let runtime = start_agent_loop(task_id, prompt.clone(), AgentRuntimeConfig::default());
    continue_agent_loop(&state, &config, &root, runtime, prompt, run_context, None)
}

#[tauri::command]
async fn resolve_agent_permission(
    app: tauri::AppHandle,
    request_id: String,
    decision: String,
    session_id: String,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        resolve_agent_permission_blocking(state, request_id, decision, session_id)
    })
    .await
    .map_err(|error| format!("agent permission resume failed to join: {error}"))?
}

fn resolve_agent_permission_blocking(
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
    session_id: String,
) -> Result<AgentState, String> {
    let mut run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let decision = parse_permission_decision(&decision).map_err(|error| error.to_string())?;
    let request_id = PermissionRequestId(request_id);
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;
    if let Some(agent_run_id) = request.metadata.get("agent_run_id") {
        run_context.insert("agent_run_id".to_string(), agent_run_id.clone());
    }
    let session_id = run_context.get("session_id").map(String::as_str);

    if request.task_id != phase16_task_id() {
        return agent_state_for_session(
            &store,
            Some("permission does not belong to the agent loop".to_string()),
            session_id,
        )
        .map_err(|error| error.to_string());
    }
    let current = agent_state_for_session(&store, None, session_id)
        .map_err(|error| error.to_string())?;
    if !current
        .pending_approvals
        .iter()
        .any(|approval| approval.request_id == request_id.0)
    {
        return agent_state_for_session(
            &store,
            Some("permission does not belong to the active session".to_string()),
            session_id,
        )
        .map_err(|error| error.to_string());
    }
    drop(store);

    let mut resolved_observations = vec![resolve_agent_permission_request(
        &state,
        &request,
        &decision,
        "local-user",
        &root,
        &run_context,
    )?];

    if matches!(&decision, PermissionDecision::AllowForSession) {
        let pending = {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            pending_agent_permissions_for_run(
                &store,
                session_id,
                run_context.get("agent_run_id").map(String::as_str),
            )
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|pending| {
                pending.action == request.action && pending.scope == request.scope
            })
            .collect::<Vec<_>>()
        };
        for pending_request in pending {
            resolved_observations.push(resolve_agent_permission_request(
                &state,
                &pending_request,
                &PermissionDecision::AllowForSession,
                "session-grant",
                &root,
                &run_context,
            )?);
        }
    }

    append_observations_to_suspended_run(
        &state,
        &session_id.clone().unwrap_or_default(),
        &resolved_observations,
    )?;

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let pending = pending_agent_permissions_for_run(
        &store,
        session_id,
        run_context.get("agent_run_id").map(String::as_str),
    )
    .map_err(|error| error.to_string())?;
    if !pending.is_empty() {
        return agent_state_for_session(&store, None, session_id)
            .map_err(|error| error.to_string());
    }

    append_event(
        &mut store,
        &request.task_id,
        EventKind::TaskStatusChanged,
        "Agent task resumed after permission",
        metadata_with_context(
            [
                ("permission_id".to_string(), request_id.0),
                (
                    "decision".to_string(),
                    permission_decision_label(&decision).to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    let events = store
        .list_by_task(&phase16_task_id())
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let prompt = latest_agent_prompt_from_active_events(&active_events)
        .unwrap_or_else(|| "Continue the agent task.".to_string());
    let transcript = agent_transcript_from_active_events(&active_events);
    drop(store);

    if let Some(session_id) = session_id {
        if let Some(suspended) = take_suspended_agent_run(&state, session_id)? {
            return continue_agent_loop(
                &state,
                &config,
                &suspended.workspace_root,
                suspended.runtime,
                suspended.prompt,
                suspended.run_context,
                suspended.collaboration.as_ref(),
            );
        }
    }

    let runtime = resume_agent_loop_from_messages(
        phase16_task_id(),
        prompt.clone(),
        transcript,
        AgentRuntimeConfig::default(),
    );
    continue_agent_loop(&state, &config, &root, runtime, prompt, run_context, None)
}

fn resolve_agent_permission_request(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    decision: &PermissionDecision,
    resolved_by: &str,
    root: &Path,
    run_context: &Metadata,
) -> Result<ResolvedToolObservation, String> {
    let request_id = request.id.clone();
    let tool_call_id = request
        .metadata
        .get("tool_call_id")
        .cloned()
        .unwrap_or_else(|| unique_id("agent-tool"));
    let tool_name = request
        .metadata
        .get("tool_name")
        .cloned()
        .unwrap_or_else(|| request.action.clone());
    let tool_input = request
        .metadata
        .get("tool_input")
        .cloned()
        .unwrap_or_default();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    store
        .resolve_permission(PermissionResolution {
            request_id: request_id.clone(),
            decision: decision.clone(),
            resolved_at_ms: current_time_millis(),
            resolved_by: resolved_by.to_string(),
        })
        .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(decision)),
        metadata_with_context(
            [
                ("permission_id".to_string(), request_id.0),
                (
                    "decision".to_string(),
                    permission_decision_label(decision).to_string(),
                ),
                ("tool".to_string(), request.action.clone()),
                ("resolved_by".to_string(), resolved_by.to_string()),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;

    let (observation, status) = if matches!(
        decision,
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession
    ) {
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId(tool_call_id.clone()),
            task_id: request.task_id.clone(),
            tool_name: tool_name.clone(),
            input_json: tool_input.clone(),
            proposed_by_model: "agent-loop".to_string(),
            metadata: Metadata::new(),
        };
        drop(store);
        let result = execute_agent_tool_invocation(state, invocation, root, run_context)?;
        let observation = observation_from_tool_result(
            &tool_name,
            tool_outcome_label(&result.status),
            &result.output,
        );
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_tool_message_event(
            &mut store,
            &request.task_id,
            &tool_call_id,
            &tool_name,
            tool_outcome_label(&result.status),
            &observation,
            Some(run_context),
        )
        .map_err(|error| error.to_string())?;
        (observation, result.status)
    } else {
        let observation = observation_from_tool_result(
            &request.action,
            "denied",
            "The user denied this tool call.",
        );
        append_event(
            &mut store,
            &request.task_id,
            EventKind::ToolCallFinished,
            "Agent tool denied",
            metadata_with_context(
                [
                    ("tool_call_id".to_string(), tool_call_id.clone()),
                    ("tool".to_string(), request.action.clone()),
                    ("status".to_string(), "denied".to_string()),
                    ("output".to_string(), observation.clone()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        append_tool_message_event(
            &mut store,
            &request.task_id,
            &tool_call_id,
            &request.action,
            "denied",
            &observation,
            Some(run_context),
        )
        .map_err(|error| error.to_string())?;
        (observation, ToolOutcomeStatus::Denied)
    };
    Ok(ResolvedToolObservation {
        call_id: agent_core::ToolCallId(tool_call_id),
        tool_name,
        input_json: tool_input,
        status,
        observation,
    })
}

#[tauri::command]
fn get_phase5_state(state: tauri::State<'_, AppState>) -> Result<Phase5State, String> {
    let root = active_workspace_root(&state)?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase5_state(&store, None, &root).map_err(|error| error.to_string())
}

#[tauri::command]
fn run_tool(
    state: tauri::State<'_, AppState>,
    input: ToolRunInput,
) -> Result<Phase5State, String> {
    let root = active_workspace_root(&state)?;
    let tool_name = input.tool_name.trim().to_string();
    let task_id = phase5_task_id();
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId(unique_id("tool")),
        task_id: task_id.clone(),
        tool_name: tool_name.clone(),
        input_json: input.input,
        proposed_by_model: "local-user".to_string(),
        metadata: Metadata::new(),
    };
    let registry = tool_registry_for_state(&state, &root)?;
    let Some(tool) = registry.get(&tool_name) else {
        return phase5_state_with_error(&state, format!("unknown tool: {tool_name}"));
    };

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_tool_proposed_event(&mut store, &invocation, None)
        .map_err(|error| error.to_string())?;

    if let Some(mut request) = tool.permission_request(&invocation) {
        request.id = PermissionRequestId(unique_id("perm"));
        request.metadata.insert("phase".to_string(), "5".to_string());
        request
            .metadata
            .insert("tool_input".to_string(), invocation.input_json.clone());
        request
            .metadata
            .insert("tool_call_id".to_string(), invocation.id.0.clone());
        request
            .metadata
            .insert("tool_name".to_string(), invocation.tool_name.clone());
        store
            .save_permission_request(request.clone(), current_time_millis())
            .map_err(|error| error.to_string())?;
        append_event(
            &mut store,
            &task_id,
            EventKind::PermissionRequested,
            format!("Permission requested for {}", request.action),
            [
                ("permission_id".to_string(), request.id.0),
                ("tool_call_id".to_string(), invocation.id.0),
                ("tool".to_string(), request.action),
                ("risk".to_string(), permission_risk_label(&request.risk).to_string()),
                ("scope".to_string(), request.scope),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;

        return phase5_state(&store, None, &root).map_err(|error| error.to_string());
    }

    execute_tool_invocation(&mut store, invocation, &root).map_err(|error| error.to_string())?;
    phase5_state(&store, None, &root).map_err(|error| error.to_string())
}

#[tauri::command]
fn resolve_tool_permission(
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
) -> Result<Phase5State, String> {
    let root = active_workspace_root(&state)?;
    let decision = parse_permission_decision(&decision).map_err(|error| error.to_string())?;
    let request_id = PermissionRequestId(request_id);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;

    store
        .resolve_permission(PermissionResolution {
            request_id: request_id.clone(),
            decision: decision.clone(),
            resolved_at_ms: current_time_millis(),
            resolved_by: "local-user".to_string(),
        })
        .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(&decision)),
        [
            ("permission_id".to_string(), request_id.0.clone()),
            (
                "decision".to_string(),
                permission_decision_label(&decision).to_string(),
            ),
            ("tool".to_string(), request.action.clone()),
        ]
        .into_iter()
        .collect(),
    )
    .map_err(|error| error.to_string())?;

    if matches!(
        decision,
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession
    ) {
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId(
                request
                    .metadata
                    .get("tool_call_id")
                    .cloned()
                    .unwrap_or_else(|| unique_id("tool")),
            ),
            task_id: request.task_id.clone(),
            tool_name: request
                .metadata
                .get("tool_name")
                .cloned()
                .unwrap_or(request.action),
            input_json: request
                .metadata
                .get("tool_input")
                .cloned()
                .unwrap_or_default(),
            proposed_by_model: "local-user".to_string(),
            metadata: Metadata::new(),
        };
        execute_tool_invocation(&mut store, invocation, &root).map_err(|error| error.to_string())?;
    } else {
        append_event(
            &mut store,
            &request.task_id,
            EventKind::ToolCallFinished,
            "Tool call denied",
            [
                (
                    "tool_call_id".to_string(),
                    request
                        .metadata
                        .get("tool_call_id")
                        .cloned()
                        .unwrap_or_default(),
                ),
                (
                    "tool".to_string(),
                    request
                        .metadata
                        .get("tool_name")
                        .cloned()
                        .unwrap_or(request.action),
                ),
                ("status".to_string(), "denied".to_string()),
                ("output".to_string(), "denied by user".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    phase5_state(&store, None, &root).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_phase6_state(state: tauri::State<'_, AppState>) -> Result<Phase6State, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase6_state(&store, None).map_err(|error| error.to_string())
}

#[tauri::command]
fn run_orchestration(
    state: tauri::State<'_, AppState>,
    input: OrchestrationRunInput,
) -> Result<Phase6State, String> {
    let prompt = input.prompt.trim().to_string();
    if prompt.is_empty() {
        return phase6_state_with_error(&state, "orchestration prompt is empty");
    }
    let Some(requested_policy) = parse_policy(input.policy.trim()) else {
        return phase6_state_with_error(&state, format!("unknown policy: {}", input.policy));
    };
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return phase6_state_with_error(&state, "Provider config is incomplete");
    }

    let mut routing_context = RoutingContext::from_prompt(&prompt, model_candidates_for_config(&config));
    if requested_policy != OrchestrationPolicy::AutoRouter {
        routing_context.user_policy_override = Some(requested_policy.clone());
    }
    let router = LearnedModelRouter::train(&[]);
    let routing_decision = router.route(&routing_context);
    let policy = if requested_policy == OrchestrationPolicy::AutoRouter {
        routing_decision.policy.clone()
    } else {
        requested_policy.clone()
    };
    let plan = default_plan(policy.clone());
    let task_id = phase6_task_id();
    let orchestration_id = unique_id("orch");
    let mut previous_outputs = Vec::new();

    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            format!("Orchestration started: {}", policy.label()),
            [
                ("orchestration_id".to_string(), orchestration_id.clone()),
                (
                    "requested_policy".to_string(),
                    requested_policy.label().to_string(),
                ),
                ("policy".to_string(), policy.label().to_string()),
                (
                    "router_model".to_string(),
                    routing_decision.model.clone(),
                ),
                (
                    "router_retrieval_mode".to_string(),
                    routing_decision.retrieval_mode.clone(),
                ),
                (
                    "router_explanation".to_string(),
                    routing_decision.explanation.clone(),
                ),
                (
                    "task_class".to_string(),
                    routing_context.task_class.label().to_string(),
                ),
                ("prompt".to_string(), prompt.clone()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    for (step_index, step) in plan.steps.iter().enumerate() {
        let step_prompt = step_prompt(&plan, step_index, &prompt, &previous_outputs)
            .ok_or_else(|| "orchestration step was missing".to_string())?;
        let model = orchestration_model_for_step(&config, &step.role, &routing_decision);
        let request_id = unique_id("model");
        let started_at_ms = current_time_millis();

        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_event(
                &mut store,
                &task_id,
                EventKind::ModelRequestStarted,
                format!("{} step started", role_label(&step.role)),
                [
                    ("orchestration_id".to_string(), orchestration_id.clone()),
                    ("request_id".to_string(), request_id.clone()),
                    ("policy".to_string(), policy.label().to_string()),
                    ("step_index".to_string(), step_index.to_string()),
                    ("role".to_string(), role_label(&step.role).to_string()),
                    ("model".to_string(), model.clone()),
                ]
                .into_iter()
                .collect(),
            )
            .map_err(|error| error.to_string())?;
        }

        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            model: model.clone(),
            embedding_model: config.model_for_role(&ModelRole::Embedder),
            timeout_seconds: 180,
        });
        let request = ModelRequest {
            role: step.role.clone(),
            messages: vec![Message {
                role: MessageRole::User,
                content: step_prompt,
                metadata: Metadata::new(),
            }],
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata: Metadata::new(),
        };

        match provider.complete_streaming(request, |_| {}) {
            Ok(response) => {
                let latency_ms = current_time_millis().saturating_sub(started_at_ms);
                previous_outputs.push(response.message.content.clone());
                let mut store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                append_event(
                    &mut store,
                    &task_id,
                    EventKind::ModelRequestFinished,
                    format!("{} step finished", role_label(&step.role)),
                    [
                        ("orchestration_id".to_string(), orchestration_id.clone()),
                        ("request_id".to_string(), request_id),
                        ("policy".to_string(), policy.label().to_string()),
                        ("step_index".to_string(), step_index.to_string()),
                        ("role".to_string(), role_label(&step.role).to_string()),
                        ("model".to_string(), model),
                        ("latency_ms".to_string(), latency_ms.to_string()),
                        ("output".to_string(), response.message.content),
                    ]
                    .into_iter()
                    .collect(),
                )
                .map_err(|error| error.to_string())?;
            }
            Err(error) => {
                let message = error.to_string();
                let mut store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                append_event(
                    &mut store,
                    &task_id,
                    EventKind::Error,
                    "Orchestration failed",
                    [
                        ("orchestration_id".to_string(), orchestration_id),
                        ("policy".to_string(), policy.label().to_string()),
                        ("step_index".to_string(), step_index.to_string()),
                        ("role".to_string(), role_label(&step.role).to_string()),
                        ("error".to_string(), message.clone()),
                    ]
                    .into_iter()
                    .collect(),
                )
                .map_err(|error| error.to_string())?;
                return phase6_state(&store, Some(message)).map_err(|error| error.to_string());
            }
        }
    }

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &task_id,
        EventKind::TaskStatusChanged,
        format!("Orchestration finished: {}", policy.label()),
        [
            ("orchestration_id".to_string(), orchestration_id),
            (
                "requested_policy".to_string(),
                requested_policy.label().to_string(),
            ),
            ("policy".to_string(), policy.label().to_string()),
            ("steps".to_string(), plan.steps.len().to_string()),
            (
                "router_model".to_string(),
                routing_decision.model.clone(),
            ),
            (
                "router_retrieval_mode".to_string(),
                routing_decision.retrieval_mode.clone(),
            ),
            (
                "router_explanation".to_string(),
                routing_decision.explanation.clone(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase6_state(&store, None).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_phase7_state(state: tauri::State<'_, AppState>) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let adapter = open_rag_adapter_for(&root)?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase7_state(&store, &adapter, Vec::new(), None, None).map_err(|error| error.to_string())
}

#[tauri::command]
fn index_workspace_rag(state: tauri::State<'_, AppState>) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let config = clone_provider_config(&state)?;
    let mut adapter = open_rag_adapter_for(&root)?;
    let (index, embedding_backend, embedding_model) = if config.is_ready() {
        let mut embedder = CloudRagEmbedder {
            config: config.clone(),
        };
        let index = index_workspace_with_embedder(&root, IndexOptions::default(), &mut embedder)
            .map_err(|error| error.to_string())?;
        (
            index,
            "cloud".to_string(),
            config.model_for_role(&ModelRole::Embedder),
        )
    } else {
        let index =
            index_workspace(&root, IndexOptions::default()).map_err(|error| error.to_string())?;
        (index, "local".to_string(), "local-hash".to_string())
    };
    let lancedb_export_path = lancedb_export_path_for(&root);
    let lancedb_records =
        export_lancedb_records_jsonl(&index, &lancedb_export_path).map_err(|error| error.to_string())?;
    let (graph_nodes, graph_edges) = index_graph_chunks(&root, index.chunks.as_slice())?;
    let stats = adapter
        .replace_all(index)
        .map_err(|error| error.to_string())?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        "Workspace indexed for RAG",
        [
            ("action".to_string(), "index".to_string()),
            ("files_indexed".to_string(), stats.files_indexed.to_string()),
            ("chunks_indexed".to_string(), stats.chunks_indexed.to_string()),
            ("indexed_at_ms".to_string(), stats.indexed_at_ms.to_string()),
            (
                "index_path".to_string(),
                rag_index_path_for(&root).display().to_string(),
            ),
            (
                "lancedb_export_path".to_string(),
                lancedb_export_path.display().to_string(),
            ),
            ("lancedb_records".to_string(), lancedb_records.to_string()),
            ("embedding_backend".to_string(), embedding_backend),
            ("embedding_model".to_string(), embedding_model),
            (
                "graph_store_path".to_string(),
                graph_store_path_for(&root).display().to_string(),
            ),
            ("graph_nodes".to_string(), graph_nodes.to_string()),
            ("graph_edges".to_string(), graph_edges.to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase7_state(&store, &adapter, Vec::new(), None, None).map_err(|error| error.to_string())
}

#[tauri::command]
fn search_rag(
    state: tauri::State<'_, AppState>,
    input: RagSearchInput,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let query = input.query.trim().to_string();
    if query.is_empty() {
        return phase7_state_with_error(&state, "RAG query is empty", Vec::new(), None);
    }

    let adapter = open_rag_adapter_for(&root)?;
    let results = adapter
        .search(&query, input.limit.unwrap_or(6))
        .map_err(|error| error.to_string())?;
    let trace = graph_rag_trace_for(&root, &adapter, &query, &results, input.limit.unwrap_or(6))?;
    let selected_results = rag_results_from_graph_trace(&trace);
    let sources = rag_sources_from_graph_trace(&trace);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_rag_retrieval_event(&mut store, "search", &query, &selected_results, Some(&trace))
        .map_err(|error| error.to_string())?;

    phase7_state(&store, &adapter, sources, None, None).map_err(|error| error.to_string())
}

#[tauri::command]
fn answer_with_rag(
    state: tauri::State<'_, AppState>,
    input: RagSearchInput,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let query = input.query.trim().to_string();
    if query.is_empty() {
        return phase7_state_with_error(&state, "RAG query is empty", Vec::new(), None);
    }

    let adapter = open_rag_adapter_for(&root)?;
    let results = adapter
        .search(&query, input.limit.unwrap_or(6))
        .map_err(|error| error.to_string())?;
    let trace = graph_rag_trace_for(&root, &adapter, &query, &results, input.limit.unwrap_or(6))?;
    let selected_results = rag_results_from_graph_trace(&trace);
    let sources = rag_sources_from_graph_trace(&trace);

    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_rag_retrieval_event(&mut store, "answer", &query, &selected_results, Some(&trace))
            .map_err(|error| error.to_string())?;
    }

    if selected_results.is_empty() {
        return phase7_state_with_error(
            &state,
            "No indexed sources matched the RAG query",
            sources,
            None,
        );
    }

    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return phase7_state_with_error(&state, "Provider config is incomplete", sources, None);
    }

    let prompt = build_grounded_answer_prompt(&query, &selected_results);
    let request_id = unique_id("rag-model");
    let model = config.model_for_role(&ModelRole::Executor);
    let task_id = phase7_task_id();
    let started_at_ms = current_time_millis();

    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &task_id,
            EventKind::ModelRequestStarted,
            format!("RAG answer request started for {model}"),
            [
                ("request_id".to_string(), request_id.clone()),
                ("provider".to_string(), "openai-compatible".to_string()),
                ("base_url".to_string(), config.base_url.clone()),
                ("model".to_string(), model.clone()),
                ("role".to_string(), "executor".to_string()),
                ("source_count".to_string(), selected_results.len().to_string()),
                ("prompt_length".to_string(), prompt.len().to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: 180,
    });
    let request = ModelRequest {
        role: ModelRole::Executor,
        messages: vec![Message {
            role: MessageRole::User,
            content: prompt,
            metadata: Metadata::new(),
        }],
        tools: Vec::new(),
        mode: ModelCallMode::Streaming,
        metadata: Metadata::new(),
    };

    match provider.complete_streaming(request, |_| {}) {
        Ok(response) => {
            let answer = response.message.content;
            let latency_ms = current_time_millis().saturating_sub(started_at_ms);
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_event(
                &mut store,
                &task_id,
                EventKind::ModelRequestFinished,
                format!("RAG answer received from {model}"),
                [
                    ("request_id".to_string(), request_id),
                    ("provider".to_string(), "openai-compatible".to_string()),
                    ("model".to_string(), model),
                    ("latency_ms".to_string(), latency_ms.to_string()),
                    ("source_count".to_string(), selected_results.len().to_string()),
                    ("answer".to_string(), answer.clone()),
                    ("output_length".to_string(), answer.len().to_string()),
                ]
                .into_iter()
                .collect(),
            )
            .map_err(|error| error.to_string())?;

            phase7_state(&store, &adapter, sources, Some(answer), None)
                .map_err(|error| error.to_string())
        }
        Err(error) => {
            let message = error.to_string();
            phase7_state_with_error(&state, message, sources, None)
        }
    }
}

#[tauri::command]
fn get_phase8_state(state: tauri::State<'_, AppState>) -> Result<Phase8State, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase8_state(&store, None).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_context_state(state: tauri::State<'_, AppState>) -> Result<ContextState, String> {
    let root = active_workspace_root(&state)?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    context_state(&store, &root, None, None)
}

#[tauri::command]
fn compact_context(state: tauri::State<'_, AppState>) -> Result<ContextState, String> {
    let root = active_workspace_root(&state)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let events = collect_context_events(&store).map_err(|error| error.to_string())?;
    let checkpoint = build_session_checkpoint_at(
        &events,
        CheckpointOptions::default(),
        current_time_millis(),
    );
    let pack = build_restore_context_pack(checkpoint);
    let checkpoint_path = write_context_checkpoint(&root, &pack.text)?;
    let mut metadata = Metadata::new();
    metadata.insert("checkpoint_id".to_string(), pack.checkpoint.id.clone());
    metadata.insert("event_count".to_string(), pack.checkpoint.event_count.to_string());
    metadata.insert("task_count".to_string(), pack.checkpoint.task_count.to_string());
    metadata.insert(
        "latest_event_ms".to_string(),
        pack.checkpoint.latest_event_ms.to_string(),
    );
    metadata.insert(
        "context_checkpoint_path".to_string(),
        checkpoint_path.display().to_string(),
    );
    if let Some(goal) = &pack.checkpoint.current_goal {
        metadata.insert("current_goal".to_string(), goal.clone());
    }

    append_event(
        &mut store,
        &phase15_task_id(),
        EventKind::TaskStatusChanged,
        "Context checkpoint compacted",
        metadata,
    )
    .map_err(|error| error.to_string())?;

    context_state(&store, &root, Some(pack), None)
}

#[tauri::command]
fn run_browser_tool(
    state: tauri::State<'_, AppState>,
    input: BrowserToolInput,
) -> Result<Phase8State, String> {
    let root = active_workspace_root(&state)?;
    let tool_name = input.tool_name.trim().to_string();
    if !is_phase8_tool(&tool_name) {
        return phase8_state_with_error(&state, format!("not a Phase 8 tool: {tool_name}"));
    }

    let task_id = phase8_task_id();
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId(unique_id("browser")),
        task_id: task_id.clone(),
        tool_name: tool_name.clone(),
        input_json: input.input,
        proposed_by_model: "local-user".to_string(),
        metadata: Metadata::new(),
    };
    let registry = tool_registry_for_state(&state, &root)?;
    let Some(tool) = registry.get(&tool_name) else {
        return phase8_state_with_error(&state, format!("unknown tool: {tool_name}"));
    };

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_tool_proposed_event(&mut store, &invocation, None)
        .map_err(|error| error.to_string())?;

    if let Some(mut request) = tool.permission_request(&invocation) {
        request.id = PermissionRequestId(unique_id("perm"));
        request.metadata.insert("phase".to_string(), "8".to_string());
        request
            .metadata
            .insert("tool_input".to_string(), invocation.input_json.clone());
        request
            .metadata
            .insert("tool_call_id".to_string(), invocation.id.0.clone());
        request
            .metadata
            .insert("tool_name".to_string(), invocation.tool_name.clone());
        store
            .save_permission_request(request.clone(), current_time_millis())
            .map_err(|error| error.to_string())?;
        append_event(
            &mut store,
            &task_id,
            EventKind::PermissionRequested,
            format!("Permission requested for {}", request.action),
            [
                ("permission_id".to_string(), request.id.0),
                ("tool_call_id".to_string(), invocation.id.0),
                ("tool".to_string(), request.action),
                (
                    "risk".to_string(),
                    permission_risk_label(&request.risk).to_string(),
                ),
                ("scope".to_string(), request.scope),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;

        return phase8_state(&store, None).map_err(|error| error.to_string());
    }

    execute_tool_invocation(&mut store, invocation, &root).map_err(|error| error.to_string())?;
    phase8_state(&store, None).map_err(|error| error.to_string())
}

#[tauri::command]
fn resolve_browser_permission(
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
) -> Result<Phase8State, String> {
    let root = active_workspace_root(&state)?;
    let decision = parse_permission_decision(&decision).map_err(|error| error.to_string())?;
    let request_id = PermissionRequestId(request_id);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;

    if request.task_id != phase8_task_id() {
        return phase8_state(&store, Some("permission does not belong to Phase 8".to_string()))
            .map_err(|error| error.to_string());
    }

    store
        .resolve_permission(PermissionResolution {
            request_id: request_id.clone(),
            decision: decision.clone(),
            resolved_at_ms: current_time_millis(),
            resolved_by: "local-user".to_string(),
        })
        .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(&decision)),
        [
            ("permission_id".to_string(), request_id.0.clone()),
            (
                "decision".to_string(),
                permission_decision_label(&decision).to_string(),
            ),
            ("tool".to_string(), request.action.clone()),
        ]
        .into_iter()
        .collect(),
    )
    .map_err(|error| error.to_string())?;

    if matches!(
        decision,
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession
    ) {
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId(
                request
                    .metadata
                    .get("tool_call_id")
                    .cloned()
                    .unwrap_or_else(|| unique_id("browser")),
            ),
            task_id: request.task_id.clone(),
            tool_name: request
                .metadata
                .get("tool_name")
                .cloned()
                .unwrap_or(request.action),
            input_json: request
                .metadata
                .get("tool_input")
                .cloned()
                .unwrap_or_default(),
            proposed_by_model: "local-user".to_string(),
            metadata: Metadata::new(),
        };
        execute_tool_invocation(&mut store, invocation, &root).map_err(|error| error.to_string())?;
    } else {
        append_event(
            &mut store,
            &request.task_id,
            EventKind::ToolCallFinished,
            "Browser tool denied",
            [
                (
                    "tool_call_id".to_string(),
                    request
                        .metadata
                        .get("tool_call_id")
                        .cloned()
                        .unwrap_or_default(),
                ),
                (
                    "tool".to_string(),
                    request
                        .metadata
                        .get("tool_name")
                        .cloned()
                        .unwrap_or(request.action),
                ),
                ("status".to_string(), "denied".to_string()),
                ("output".to_string(), "denied by user".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    phase8_state(&store, None).map_err(|error| error.to_string())
}

pub fn run() {
    install_startup_panic_log();
    if let Err(error) = migrate_legacy_app_data() {
        append_startup_log(&format!("legacy data migration failed: {error}"));
    }
    append_startup_log(&format!(
        "starting Cindx {} on {} with data root {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        app_data_root().display()
    ));
    let mut store = match open_app_store() {
        Ok(store) => store,
        Err(error) => {
            append_startup_log(&format!(
                "persistent state unavailable; using in-memory state: {error}"
            ));
            SqliteStore::in_memory().unwrap_or_else(|memory_error| {
                panic!(
                    "failed to open persistent state ({error}) and in-memory state ({memory_error})"
                )
            })
        }
    };
    if let Err(error) = redact_persisted_events(&mut store) {
        eprintln!("failed to redact persisted Cindx history: {error}");
    }
    let provider_config = load_provider_config();
    let mcp_catalog = McpCatalogService::load(mcp_config_path(), mcp_catalog_cache_path());
    let mut workspace_config = load_workspace_config();
    let sidecar_config = load_sidecar_config();
    let project_session_config = load_project_session_config(&workspace_config.root);
    if let Some(project) = project_session_config.active_project() {
        if let Ok(root) = validate_workspace_root(&project.root) {
            workspace_config.root = root;
        }
    }
    for path in [
        context_checkpoint_path_for(&workspace_config.root),
        agent_trace_export_path_for(&workspace_config.root),
    ] {
        if let Err(error) = redact_existing_text_artifact(&path) {
            eprintln!("failed to redact {}: {error}", path.display());
        }
    }
    apply_sidecar_env(&sidecar_config);
    if std::env::var("CINDX_STARTUP_PROBE")
        .map(|value| config_bool(&value))
        .unwrap_or(false)
    {
        append_startup_log("startup probe completed");
        return;
    }

    let app = tauri::Builder::default()
        .manage(AppState {
            store: Mutex::new(store),
            provider_config: Mutex::new(provider_config),
            workspace_config: Mutex::new(workspace_config),
            sidecar_config: Mutex::new(sidecar_config),
            project_session_config: Mutex::new(project_session_config),
            mcp_catalog: Mutex::new(mcp_catalog),
            suspended_agent_runs: Mutex::new(BTreeMap::new()),
            allow_exit: AtomicBool::new(false),
            quit_prompt_active: AtomicBool::new(false),
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !confirm_application_exit(window.app_handle()) {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_runtime_status,
            get_sidecar_state,
            save_sidecar_config,
            get_mcp_state,
            save_mcp_servers,
            upsert_mcp_server,
            update_mcp_server_policy,
            remove_mcp_server,
            refresh_mcp_server,
            get_skill_state,
            refresh_skills,
            save_skill_preference,
            get_project_session_state,
            create_project,
            create_session,
            rename_session,
            fork_session,
            archive_session,
            restore_session,
            delete_session,
            select_project,
            select_session,
            save_workspace_root,
            get_phase3_state,
            request_mock_permission,
            resolve_permission,
            get_phase4_state,
            save_provider_config,
            list_provider_models,
            send_model_prompt,
            get_agent_state,
            get_agent_trace_state,
            export_agent_trace_jsonl,
            run_agent_task,
            cancel_agent_task,
            retry_agent_task,
            resolve_agent_permission,
            get_phase5_state,
            run_tool,
            resolve_tool_permission,
            get_phase6_state,
            run_orchestration,
            get_phase7_state,
            index_workspace_rag,
            search_rag,
            answer_with_rag,
            get_phase8_state,
            get_context_state,
            compact_context,
            read_artifact_image,
            run_browser_tool,
            resolve_browser_permission
        ])
        .build(tauri::generate_context!())
        .expect("error while building Cindx desktop app");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if !confirm_application_exit(app_handle) {
                api.prevent_exit();
            }
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuitConfirmation {
    confirmed: bool,
    suppress_future: bool,
}

fn confirm_application_exit(app_handle: &tauri::AppHandle) -> bool {
    let state = app_handle.state::<AppState>();
    if state.allow_exit.load(Ordering::SeqCst) {
        return true;
    }
    if quit_confirmation_suppressed(app_handle) {
        state.allow_exit.store(true, Ordering::SeqCst);
        return true;
    }
    if state.quit_prompt_active.swap(true, Ordering::SeqCst) {
        return false;
    }

    let confirmation = show_native_quit_confirmation();
    state.quit_prompt_active.store(false, Ordering::SeqCst);
    if !confirmation.confirmed {
        return false;
    }
    if confirmation.suppress_future {
        if let Err(error) = persist_quit_confirmation_suppression(app_handle) {
            eprintln!("failed to save quit confirmation preference: {error}");
        }
    }
    state.allow_exit.store(true, Ordering::SeqCst);
    true
}

fn quit_confirmation_preference_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_handle
        .path()
        .app_config_dir()
        .map(|path| path.join("preferences.conf"))
        .map_err(|error| format!("failed to resolve app config directory: {error}"))
}

fn quit_confirmation_suppressed(app_handle: &tauri::AppHandle) -> bool {
    let Ok(path) = quit_confirmation_preference_path(app_handle) else {
        return false;
    };
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    quit_confirmation_suppressed_text(&text)
}

fn quit_confirmation_suppressed_text(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim() == "skip_quit_confirmation=true")
}

fn persist_quit_confirmation_suppression(
    app_handle: &tauri::AppHandle,
) -> Result<(), String> {
    let path = quit_confirmation_preference_path(app_handle)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create app config directory: {error}"))?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|error| format!("failed to open quit preference: {error}"))?;
    file.write_all(b"skip_quit_confirmation=true\n")
        .map_err(|error| format!("failed to write quit preference: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure quit preference: {error}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn show_native_quit_confirmation() -> QuitConfirmation {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSControlStateValueOn,
    };
    use objc2_foundation::NSString;

    let Some(main_thread) = MainThreadMarker::new() else {
        return QuitConfirmation {
            confirmed: false,
            suppress_future: false,
        };
    };
    let alert = NSAlert::new(main_thread);
    alert.setAlertStyle(NSAlertStyle::Informational);
    alert.setMessageText(&NSString::from_str("Quit Cindx?"));
    alert.setInformativeText(&NSString::from_str(
        "Any running agent work will stop when the app quits.",
    ));
    alert.addButtonWithTitle(&NSString::from_str("Quit Cindx"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    alert.setShowsSuppressionButton(true);
    if let Some(button) = alert.suppressionButton() {
        button.setTitle(&NSString::from_str("Don't ask again"));
    }

    let response = alert.runModal();
    let suppress_future = alert
        .suppressionButton()
        .is_some_and(|button| button.state() == NSControlStateValueOn);
    QuitConfirmation {
        confirmed: response == NSAlertFirstButtonReturn,
        suppress_future,
    }
}

#[cfg(not(target_os = "macos"))]
fn show_native_quit_confirmation() -> QuitConfirmation {
    QuitConfirmation {
        confirmed: true,
        suppress_future: false,
    }
}

#[tauri::command]
fn read_artifact_image(state: tauri::State<'_, AppState>, path: String) -> Result<String, String> {
    let workspace_root = state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))?
        .root
        .clone();
    let requested = PathBuf::from(&path);
    let requested = if requested.is_absolute() {
        requested
    } else {
        workspace_root.join(requested)
    };
    let canonical_root = fs::canonicalize(&workspace_root)
        .map_err(|error| format!("failed to resolve workspace root: {error}"))?;
    let canonical_path = fs::canonicalize(&requested)
        .map_err(|error| format!("failed to resolve artifact image: {error}"))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err("artifact image must be inside the active workspace".to_string());
    }

    let metadata = fs::metadata(&canonical_path)
        .map_err(|error| format!("failed to inspect artifact image: {error}"))?;
    if metadata.len() > 24 * 1024 * 1024 {
        return Err("artifact image exceeds the 24 MB preview limit".to_string());
    }

    let extension = canonical_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => return Err("artifact is not a supported preview image".to_string()),
    };
    let bytes = fs::read(&canonical_path)
        .map_err(|error| format!("failed to read artifact image: {error}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

fn request_mock_permission_in_store(store: &mut SqliteStore) -> Result<Phase3State, StorageError> {
    let task_id = phase3_task_id();
    let requested_at_ms = current_time_millis();
    let request_id = PermissionRequestId(unique_id("perm"));
    let mut metadata = Metadata::new();
    metadata.insert("command".to_string(), "echo phase-3".to_string());
    metadata.insert("cwd".to_string(), workspace_root().display().to_string());

    let request = PermissionRequest {
        id: request_id.clone(),
        task_id: task_id.clone(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "Run a harmless mock command to verify the permission gate.".to_string(),
        scope: workspace_root().display().to_string(),
        metadata,
    };

    append_event(
        store,
        &task_id,
        EventKind::ToolCallProposed,
        "Mock shell.run proposed",
        [
            ("tool".to_string(), request.action.clone()),
            ("permission_id".to_string(), request_id.0.clone()),
        ]
        .into_iter()
        .collect(),
    )?;

    store.save_permission_request(request.clone(), requested_at_ms)?;

    append_event(
        store,
        &task_id,
        EventKind::PermissionRequested,
        format!("Permission requested for {}", request.action),
        [
            ("permission_id".to_string(), request.id.0.clone()),
            ("risk".to_string(), permission_risk_label(&request.risk).to_string()),
            ("scope".to_string(), request.scope),
        ]
        .into_iter()
        .collect(),
    )?;

    phase3_state(store)
}

fn resolve_permission_in_store(
    store: &mut SqliteStore,
    request_id: &str,
    decision: &str,
) -> Result<Phase3State, StorageError> {
    let request_id = PermissionRequestId(request_id.to_string());
    let request = store
        .get_permission_request(&request_id)?
        .ok_or_else(|| StorageError::new("permission request not found"))?;
    let decision = parse_permission_decision(decision)?;
    let resolved_at_ms = current_time_millis();

    store.resolve_permission(PermissionResolution {
        request_id: request_id.clone(),
        decision: decision.clone(),
        resolved_at_ms,
        resolved_by: "local-user".to_string(),
    })?;

    append_event(
        store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(&decision)),
        [
            ("permission_id".to_string(), request_id.0),
            (
                "decision".to_string(),
                permission_decision_label(&decision).to_string(),
            ),
            ("tool".to_string(), request.action),
        ]
        .into_iter()
        .collect(),
    )?;

    phase3_state(store)
}

fn phase3_state(store: &SqliteStore) -> Result<Phase3State, StorageError> {
    let task_id = phase3_task_id();
    let audits = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id)
        .collect::<Vec<_>>();
    let timeline = store
        .list_by_task(&task_id)?
        .into_iter()
        .map(|event| timeline_entry(event, &audits))
        .collect();
    let permissions = audits.into_iter().map(permission_audit).collect();

    Ok(Phase3State {
        timeline,
        permissions,
    })
}

fn phase4_state(
    store: &SqliteStore,
    config: &ProviderConfig,
    last_error: Option<String>,
) -> Result<Phase4State, StorageError> {
    let task_id = phase4_task_id();
    let events = store.list_by_task(&task_id)?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let messages = events
        .iter()
        .filter_map(message_view_from_event)
        .collect::<Vec<_>>();

    Ok(Phase4State {
        provider: provider_config_state(config),
        timeline,
        messages,
        last_error,
    })
}

fn phase5_state(
    store: &SqliteStore,
    last_error: Option<String>,
    workspace_root: &Path,
) -> Result<Phase5State, StorageError> {
    let task_id = phase5_task_id();
    let events = store.list_by_task(&task_id)?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let results = events.iter().filter_map(tool_run_from_event).collect();
    let pending_approvals = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id && audit.resolution.is_none())
        .filter_map(tool_approval_from_audit)
        .collect();
    let tools = ToolRegistry::with_workspace_tools(workspace_root.to_path_buf())
        .specs()
        .into_iter()
        .map(|spec| ToolSpecView {
            name: spec.name,
            description: spec.description,
            risk: tool_risk_label(&spec.risk).to_string(),
            input_schema: spec.input_schema_json,
        })
        .collect();

    Ok(Phase5State {
        timeline,
        tools,
        pending_approvals,
        results,
        last_error,
    })
}

fn phase6_state(
    store: &SqliteStore,
    last_error: Option<String>,
) -> Result<Phase6State, StorageError> {
    let events = store.list_by_task(&phase6_task_id())?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let steps = events
        .iter()
        .filter_map(orchestration_step_from_event)
        .collect();

    Ok(Phase6State {
        timeline,
        steps,
        last_error,
    })
}

fn phase7_state(
    store: &SqliteStore,
    adapter: &FileRagAdapter,
    sources: Vec<RagSourceView>,
    answer: Option<String>,
    last_error: Option<String>,
) -> Result<Phase7State, StorageError> {
    let events = store.list_by_task(&phase7_task_id())?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let answer = answer.or_else(|| events.iter().rev().find_map(rag_answer_from_event));

    Ok(Phase7State {
        timeline,
        stats: rag_stats_view(adapter.stats()),
        sources,
        answer,
        last_error,
    })
}

fn phase7_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
    sources: Vec<RagSourceView>,
    answer: Option<String>,
) -> Result<Phase7State, String> {
    let message = message.into();
    let root = active_workspace_root(state)?;
    let adapter = open_rag_adapter_for(&root)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::Error,
        "RAG request failed",
        [("error".to_string(), message.clone())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase7_state(&store, &adapter, sources, answer, Some(message)).map_err(|error| error.to_string())
}

fn phase8_state(
    store: &SqliteStore,
    last_error: Option<String>,
) -> Result<Phase8State, StorageError> {
    let task_id = phase8_task_id();
    let events = store.list_by_task(&task_id)?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let observations = events
        .iter()
        .filter_map(browser_observation_from_event)
        .collect();
    let pending_approvals = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id && audit.resolution.is_none())
        .filter_map(tool_approval_from_audit)
        .collect();

    Ok(Phase8State {
        timeline,
        pending_approvals,
        observations,
        last_error,
    })
}

fn context_state(
    store: &SqliteStore,
    workspace_root: &Path,
    pack: Option<RestoreContextPack>,
    last_error: Option<String>,
) -> Result<ContextState, String> {
    let audits = store
        .list_permission_audits()
        .map_err(|error| error.to_string())?;
    let timeline = store
        .list_by_task(&phase15_task_id())
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|event| timeline_entry(event, &audits))
        .collect();
    let checkpoint = match pack {
        Some(pack) => Some(context_checkpoint_view_from_pack(
            pack,
            Some(context_checkpoint_path_for(workspace_root)),
        )),
        None => Some(live_context_checkpoint_view(store, workspace_root)?),
    };

    Ok(ContextState {
        timeline,
        checkpoint,
        last_error,
    })
}

fn live_context_checkpoint_view(
    store: &SqliteStore,
    workspace_root: &Path,
) -> Result<ContextCheckpointView, String> {
    let events = collect_context_events(store).map_err(|error| error.to_string())?;
    let checkpoint = build_session_checkpoint_at(
        &events,
        CheckpointOptions::default(),
        current_time_millis(),
    );
    let mut pack = build_restore_context_pack(checkpoint);
    let checkpoint_path = context_checkpoint_path_for(workspace_root);
    let path = if checkpoint_path.exists() {
        if let Ok(text) = fs::read_to_string(&checkpoint_path) {
            if !text.trim().is_empty() {
                pack.text = text;
            }
        }
        Some(checkpoint_path)
    } else {
        None
    };

    Ok(context_checkpoint_view_from_pack(pack, path))
}

fn context_checkpoint_view_from_pack(
    pack: RestoreContextPack,
    path: Option<PathBuf>,
) -> ContextCheckpointView {
    let SessionCheckpoint {
        id,
        generated_at_ms,
        event_count,
        task_count,
        latest_event_ms,
        current_goal,
        completed_steps,
        pending_steps,
        decisions,
        file_changes,
        commands_run,
        tool_results,
        retrievals,
        artifacts,
        errors,
        next_actions,
    } = pack.checkpoint;

    ContextCheckpointView {
        id,
        generated_at_ms,
        event_count,
        task_count,
        latest_event_ms,
        current_goal: current_goal.map(|value| redact_sensitive_text(&value)),
        completed_steps: redact_string_list(completed_steps),
        pending_steps: redact_string_list(pending_steps),
        decisions: redact_string_list(decisions),
        file_changes: redact_string_list(file_changes),
        commands_run: redact_string_list(commands_run),
        tool_results: redact_string_list(tool_results),
        retrievals: redact_string_list(retrievals),
        artifacts: redact_string_list(artifacts),
        errors: redact_string_list(errors),
        next_actions: redact_string_list(next_actions),
        path: path.map(|path| path.display().to_string()),
        restore_pack: redact_sensitive_text(&pack.text),
    }
}

fn redact_string_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| redact_sensitive_text(&value))
        .collect()
}

fn collect_context_events(store: &SqliteStore) -> Result<Vec<Event>, StorageError> {
    let mut events = Vec::new();
    for task_id in context_task_ids() {
        events.extend(store.list_by_task(&task_id)?.into_iter().map(redact_event));
    }
    events.sort_by(|left, right| {
        left.timestamp_ms
            .cmp(&right.timestamp_ms)
            .then(left.sequence.cmp(&right.sequence))
            .then(left.task_id.0.cmp(&right.task_id.0))
            .then(left.id.0.cmp(&right.id.0))
    });
    Ok(events)
}

fn write_context_checkpoint(workspace_root: &Path, text: &str) -> Result<PathBuf, String> {
    let path = context_checkpoint_path_for(workspace_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create context checkpoint directory: {error}"))?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|error| format!("failed to open context checkpoint: {error}"))?;
    let redacted_text = redact_sensitive_text(text);
    file.write_all(redacted_text.as_bytes())
        .map_err(|error| format!("failed to write context checkpoint: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure context checkpoint: {error}"))?;

    Ok(path)
}

fn write_agent_trace_jsonl(
    workspace_root: &Path,
    store: &SqliteStore,
    session_id: Option<&str>,
) -> Result<PathBuf, String> {
    let trace = agent_trace_state_for_session(store, None, None, session_id)
        .map_err(|error| error.to_string())?;
    let path = agent_trace_export_path_for(workspace_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create trace export directory: {error}"))?;
    }

    let mut lines = Vec::new();
    for turn in &trace.turns {
        for step in &turn.steps {
            lines.push(agent_trace_step_json(&trace, turn, step));
        }
    }
    let text = if lines.is_empty() {
        agent_trace_empty_json(&trace)
    } else {
        format!("{}\n", lines.join("\n"))
    };

    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|error| format!("failed to open trace export: {error}"))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("failed to write trace export: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure trace export: {error}"))?;

    Ok(path)
}

fn agent_trace_step_json(
    trace: &AgentTraceState,
    turn: &AgentTraceTurnView,
    step: &AgentTraceStepView,
) -> String {
    format!(
        "{{\"trace_id\":\"{}\",\"run_id\":\"{}\",\"task_id\":\"{}\",\"turn_index\":{},\"turn_status\":\"{}\",\"step_id\":\"{}\",\"parent_id\":{},\"sequence\":{},\"kind\":\"{}\",\"label\":\"{}\",\"status\":\"{}\",\"started_at_ms\":{},\"finished_at_ms\":{},\"latency_ms\":{},\"model\":{},\"tool_name\":{},\"request_id\":{},\"tool_call_id\":{},\"permission_id\":{},\"input_preview\":{},\"output_preview\":{},\"artifact_path\":{},\"detail\":\"{}\",\"metadata\":{}}}",
        trace_json_escape(&trace.trace_id),
        trace_json_escape(&trace.run_id),
        trace_json_escape(&trace.task_id),
        turn.index,
        trace_json_escape(&turn.status),
        trace_json_escape(&step.id),
        trace_optional_string_json(step.parent_id.as_deref()),
        step.sequence,
        trace_json_escape(&step.kind),
        trace_json_escape(&step.label),
        trace_json_escape(&step.status),
        step.started_at_ms,
        trace_optional_u64_json(step.finished_at_ms),
        trace_optional_u64_json(step.latency_ms),
        trace_optional_string_json(step.model.as_deref()),
        trace_optional_string_json(step.tool_name.as_deref()),
        trace_optional_string_json(step.request_id.as_deref()),
        trace_optional_string_json(step.tool_call_id.as_deref()),
        trace_optional_string_json(step.permission_id.as_deref()),
        trace_optional_string_json(step.input_preview.as_deref()),
        trace_optional_string_json(step.output_preview.as_deref()),
        trace_optional_string_json(step.artifact_path.as_deref()),
        trace_json_escape(&step.detail),
        trace_metadata_json(&step.metadata)
    )
}

fn agent_trace_empty_json(trace: &AgentTraceState) -> String {
    format!(
        "{{\"trace_id\":\"{}\",\"run_id\":\"{}\",\"task_id\":\"{}\",\"kind\":\"empty\",\"status\":\"{}\",\"step_count\":0}}\n",
        trace_json_escape(&trace.trace_id),
        trace_json_escape(&trace.run_id),
        trace_json_escape(&trace.task_id),
        trace_json_escape(&trace.status)
    )
}

fn trace_metadata_json(metadata: &Metadata) -> String {
    format!(
        "{{{}}}",
        metadata
            .iter()
            .map(|(key, value)| format!(
                "\"{}\":\"{}\"",
                trace_json_escape(key),
                trace_json_escape(value)
            ))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn trace_optional_string_json(value: Option<&str>) -> String {
    value
        .map(|value| format!("\"{}\"", trace_json_escape(value)))
        .unwrap_or_else(|| "null".to_string())
}

fn trace_optional_u64_json(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn trace_json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other if other.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

#[derive(Debug, Clone)]
struct AgentCollaboration {
    id: String,
    policy: String,
    planner_output: String,
}

fn build_collaboration_planner_prompt(prompt: &str, history: &[Message]) -> String {
    let recent_context = history
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|message| {
            format!(
                "{}: {}",
                message_role_label(&message.role),
                truncate_for_collaboration(&message.content, 1_200)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are the planning member of a multi-model Cindx team. Produce a concise, checkable execution plan for the executor. Identify assumptions, required evidence, tool needs, and likely failure modes. Do not answer the user directly.\n\nUser request:\n{}\n\nRecent session context:\n{}",
        prompt,
        if recent_context.is_empty() {
            "(none)"
        } else {
            &recent_context
        }
    )
}

fn synthesize_agent_answer(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    executor_answer: &str,
    run_context: &Metadata,
    collaboration: &AgentCollaboration,
) -> Result<String, String> {
    let evidence = runtime
        .messages
        .iter()
        .rev()
        .filter(|message| matches!(message.role, MessageRole::Tool))
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|message| truncate_for_collaboration(&message.content, 1_500))
        .collect::<Vec<_>>()
        .join("\n\n");
    let review_prompt = format!(
        "You are the independent reviewer in a multi-model Cindx team. Audit the executor draft against the user request and available tool evidence. Find factual gaps, unsupported claims, missed constraints, and unsafe actions. Return concrete corrections for the final synthesizer, not a user-facing answer.\n\nUser request:\n{}\n\nPlanner guidance:\n{}\n\nExecutor draft:\n{}\n\nTool evidence:\n{}",
        prompt,
        if collaboration.planner_output.is_empty() {
            "(planner unavailable)"
        } else {
            &collaboration.planner_output
        },
        truncate_for_collaboration(executor_answer, 12_000),
        if evidence.is_empty() { "(none)" } else { &evidence }
    );
    let review = run_collaboration_stage(
        state,
        config,
        &runtime.task_id,
        run_context,
        &collaboration.id,
        "reviewer",
        ModelRole::Reviewer,
        &config.model_for_role(&ModelRole::Reviewer),
        review_prompt,
    )
    .unwrap_or_else(|error| format!("Reviewer unavailable: {error}"));
    let synthesis_prompt = format!(
        "You are the final synthesizer in a multi-model Cindx team. Produce the strongest possible final response to the user using the executor draft, reviewer corrections, planner intent, and tool evidence. Resolve disagreements using evidence. Do not mention the internal pipeline. Be precise, complete, and concise; never claim work that the evidence does not support.\n\nCollaboration policy: {}\n\nUser request:\n{}\n\nPlanner guidance:\n{}\n\nExecutor draft:\n{}\n\nReviewer corrections:\n{}\n\nTool evidence:\n{}",
        collaboration.policy,
        prompt,
        if collaboration.planner_output.is_empty() {
            "(planner unavailable)"
        } else {
            &collaboration.planner_output
        },
        truncate_for_collaboration(executor_answer, 12_000),
        truncate_for_collaboration(&review, 8_000),
        if evidence.is_empty() { "(none)" } else { &evidence }
    );
    let answer = run_collaboration_stage(
        state,
        config,
        &runtime.task_id,
        run_context,
        &collaboration.id,
        "synthesizer",
        ModelRole::Summarizer,
        &config.model_for_role(&ModelRole::Summarizer),
        synthesis_prompt,
    )?;
    let answer = answer.trim().to_string();
    if answer.is_empty() {
        Err("synthesizer returned an empty answer".to_string())
    } else {
        Ok(answer)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_collaboration_stage(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: ModelRole,
    model: &str,
    prompt: String,
) -> Result<String, String> {
    let request_id = unique_id("collaboration-model");
    let started_at_ms = current_time_millis();
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::ModelRequestStarted,
            format!("Collaboration {stage} started"),
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    ("request_id".to_string(), request_id.clone()),
                    ("stage".to_string(), stage.to_string()),
                    ("role".to_string(), role_label(&role).to_string()),
                    ("model".to_string(), model.to_string()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: model.to_string(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: 180,
    });
    let response = provider.complete(ModelRequest {
        role: role.clone(),
        messages: vec![
            Message {
                role: MessageRole::System,
                content: config.agent_system_prompt.clone(),
                metadata: Metadata::new(),
            },
            Message {
                role: MessageRole::User,
                content: prompt,
                metadata: Metadata::new(),
            },
        ],
        tools: Vec::new(),
        mode: ModelCallMode::NonStreaming,
        metadata: Metadata::new(),
    });
    let latency_ms = current_time_millis().saturating_sub(started_at_ms);
    match response {
        Ok(response) => {
            let mut metadata = [
                ("collaboration_id".to_string(), collaboration_id.to_string()),
                ("request_id".to_string(), request_id),
                ("stage".to_string(), stage.to_string()),
                ("role".to_string(), role_label(&role).to_string()),
                ("model".to_string(), model.to_string()),
                ("latency_ms".to_string(), latency_ms.to_string()),
                ("output".to_string(), response.message.content.clone()),
                ("status".to_string(), "completed".to_string()),
            ]
            .into_iter()
            .collect::<Metadata>();
            for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
                if let Some(value) = response.metadata.get(key) {
                    metadata.insert(key.to_string(), value.clone());
                }
            }
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_event(
                &mut store,
                task_id,
                EventKind::ModelRequestFinished,
                format!("Collaboration {stage} finished"),
                metadata_with_context(metadata, run_context),
            )
            .map_err(|error| error.to_string())?;
            Ok(response.message.content)
        }
        Err(error) => {
            let message = error.to_string();
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_event(
                &mut store,
                task_id,
                EventKind::ModelRequestFinished,
                format!("Collaboration {stage} unavailable"),
                metadata_with_context(
                    [
                        ("collaboration_id".to_string(), collaboration_id.to_string()),
                        ("request_id".to_string(), request_id),
                        ("stage".to_string(), stage.to_string()),
                        ("role".to_string(), role_label(&role).to_string()),
                        ("model".to_string(), model.to_string()),
                        ("latency_ms".to_string(), latency_ms.to_string()),
                        ("status".to_string(), "degraded".to_string()),
                        ("error".to_string(), message.clone()),
                    ]
                    .into_iter()
                    .collect(),
                    run_context,
                ),
            )
            .map_err(|error| error.to_string())?;
            Err(message)
        }
    }
}

fn truncate_for_collaboration(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
}

fn continue_agent_loop(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    mut runtime: agent_runtime::AgentLoopState,
    prompt: String,
    run_context: Metadata,
    collaboration: Option<&AgentCollaboration>,
) -> Result<AgentState, String> {
    let session_id = run_context.get("session_id").map(String::as_str);
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: config.model_for_role(&ModelRole::Executor),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: 180,
    });
    let registry = tool_registry_for_state(state, workspace_root)?;
    let tools = registry
        .exposure_plan(&prompt, config.context_window_tokens)
        .inline;

    loop {
        let request = model_request_for_turn_with_system_prompt(
            &runtime,
            &tools,
            Some(&config.agent_system_prompt),
        );
        let request_id = unique_id("agent-model");
        let started_at_ms = current_time_millis();
        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            if agent_task_is_cancelled(&store, session_id).map_err(|error| error.to_string())? {
                return agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string());
            }
            append_event(
                &mut store,
                &runtime.task_id,
                EventKind::ModelRequestStarted,
                "Agent model turn started",
                metadata_with_context(
                    [
                        ("request_id".to_string(), request_id.clone()),
                        ("turn".to_string(), runtime.turn.to_string()),
                        (
                            "model".to_string(),
                            config.model_for_role(&ModelRole::Executor),
                        ),
                        ("tool_count".to_string(), tools.len().to_string()),
                        ("prompt".to_string(), prompt.clone()),
                    ]
                    .into_iter()
                    .collect(),
                    &run_context,
                ),
            )
            .map_err(|error| error.to_string())?;
        }

        let mut response = match provider.complete(request) {
            Ok(response) => response,
            Err(error) => {
                return agent_state_with_error_in_context(
                    state,
                    &run_context,
                    format!("Agent model call failed: {error}"),
                );
            }
        };
        let should_synthesize = collaboration.is_some()
            && response.tool_calls.is_empty()
            && !response.message.content.trim().is_empty();
        if should_synthesize {
            response
                .message
                .metadata
                .insert("internal".to_string(), "true".to_string());
            response.message.metadata.insert(
                "collaboration_stage".to_string(),
                "executor_draft".to_string(),
            );
        }
        let latency_ms = current_time_millis().saturating_sub(started_at_ms);
        let output_length = response.message.content.len();
        let tool_call_count = response.tool_calls.len();
        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            if agent_task_is_cancelled(&store, session_id).map_err(|error| error.to_string())? {
                return agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string());
            }
            let mut metadata = Metadata::new();
            metadata.insert("request_id".to_string(), request_id);
            metadata.insert("latency_ms".to_string(), latency_ms.to_string());
            metadata.insert("output_length".to_string(), output_length.to_string());
            metadata.insert("tool_calls".to_string(), tool_call_count.to_string());
            for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
                if let Some(value) = response.metadata.get(key) {
                    metadata.insert(key.to_string(), value.clone());
                }
            }
            if let Some(raw_tool_calls_json) = response.raw_tool_calls_json.clone() {
                metadata.insert("raw_tool_calls_json".to_string(), raw_tool_calls_json);
            }
            append_event(
                &mut store,
                &runtime.task_id,
                EventKind::ModelRequestFinished,
                "Agent model turn finished",
                metadata_with_context(metadata, &run_context),
            )
            .map_err(|error| error.to_string())?;
        }

        let previous_message_count = runtime.messages.len();
        let advance = advance_with_model_response(&mut runtime, response, &tools);
        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            persist_new_runtime_messages(
                &mut store,
                &runtime.task_id,
                &runtime.messages,
                previous_message_count,
                &run_context,
            )
            .map_err(|error| error.to_string())?;
        }

        match advance {
            AgentAdvance::Completed { answer } => {
                clear_suspended_agent_run_for_context(state, &run_context)?;
                let final_answer = if let Some(collaboration) = collaboration {
                    synthesize_agent_answer(
                        state,
                        config,
                        &runtime,
                        &prompt,
                        &answer,
                        &run_context,
                        collaboration,
                    )
                    .unwrap_or_else(|_| answer.clone())
                } else {
                    answer.clone()
                };
                let mut store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                if collaboration.is_some() {
                    append_message_event_with_metadata(
                        &mut store,
                        &runtime.task_id,
                        MessageRole::Assistant,
                        &final_answer,
                        metadata_with_context(
                            [
                                ("collaboration_final".to_string(), "true".to_string()),
                                (
                                    "model".to_string(),
                                    config.model_for_role(&ModelRole::Summarizer),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                            &run_context,
                        ),
                    )
                    .map_err(|error| error.to_string())?;
                }
                append_event(
                    &mut store,
                    &runtime.task_id,
                    EventKind::TaskStatusChanged,
                    "Agent task completed",
                    metadata_with_context(
                        [
                            ("answer_length".to_string(), final_answer.len().to_string()),
                            (
                                "collaboration".to_string(),
                                collaboration.is_some().to_string(),
                            ),
                        ]
                            .into_iter()
                            .collect(),
                        &run_context,
                    ),
                )
                .map_err(|error| error.to_string())?;
                return agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string());
            }
            AgentAdvance::Failed { message } => {
                clear_suspended_agent_run_for_context(state, &run_context)?;
                return agent_state_with_error_in_context(state, &run_context, message);
            }
            AgentAdvance::ToolCalls { calls } => {
                let mut waiting_for_permission = false;
                for call in calls {
                    let invocation = tool_invocation_from_request(&runtime.task_id, &call);
                    let mut store = state
                        .store
                        .lock()
                        .map_err(|error| format!("store lock poisoned: {error}"))?;
                    append_tool_proposed_event(&mut store, &invocation, Some(&run_context))
                        .map_err(|error| error.to_string())?;

                    if repeated_tool_failure_count(
                        &runtime,
                        &call.tool_name,
                        &call.input,
                    ) >= MAX_IDENTICAL_TOOL_FAILURES
                    {
                        let observation = observation_from_tool_result(
                            &call.tool_name,
                            "failed",
                            "Cindx blocked this identical tool call after repeated failures. Change the arguments or use a different approach.",
                        );
                        append_tool_finished_event(
                            &mut store,
                            &runtime.task_id,
                            &call.call_id.0,
                            &call.tool_name,
                            "failed",
                            &observation,
                            [("failure_code".to_string(), "repeated_call_blocked".to_string())]
                                .into_iter()
                                .collect(),
                            Some(&run_context),
                        )
                        .map_err(|error| error.to_string())?;
                        let previous_message_count = runtime.messages.len();
                        append_tool_observation(
                            &mut runtime,
                            call.call_id.clone(),
                            &observation,
                        );
                        persist_new_runtime_messages(
                            &mut store,
                            &runtime.task_id,
                            &runtime.messages,
                            previous_message_count,
                            &run_context,
                        )
                        .map_err(|error| error.to_string())?;
                        continue;
                    }

                    let registry = tool_registry_for_state(state, workspace_root)?;
                    let Some(tool) = registry.get(&call.tool_name) else {
                        let observation = observation_from_tool_result(
                            &call.tool_name,
                            "failed",
                            "Unknown tool requested by model.",
                        );
                        append_tool_finished_event(
                            &mut store,
                            &runtime.task_id,
                            &call.call_id.0,
                            &call.tool_name,
                            "failed",
                            &observation,
                            Metadata::new(),
                            Some(&run_context),
                        )
                        .map_err(|error| error.to_string())?;
                        let previous_message_count = runtime.messages.len();
                        append_tool_observation(
                            &mut runtime,
                            call.call_id.clone(),
                            &observation,
                        );
                        persist_new_runtime_messages(
                            &mut store,
                            &runtime.task_id,
                            &runtime.messages,
                            previous_message_count,
                            &run_context,
                        )
                        .map_err(|error| error.to_string())?;
                        continue;
                    };

                    if let Some(mut request) = tool.permission_request(&invocation) {
                        request.id = PermissionRequestId(unique_id("agent-perm"));
                        request.metadata.insert("phase".to_string(), "16".to_string());
                        request
                            .metadata
                            .entry("tool_input".to_string())
                            .or_insert_with(|| invocation.input_json.clone());
                        request
                            .metadata
                            .insert("tool_call_id".to_string(), invocation.id.0.clone());
                        request
                            .metadata
                            .entry("tool_name".to_string())
                            .or_insert_with(|| invocation.tool_name.clone());
                        request
                            .metadata
                            .insert("agent_prompt".to_string(), prompt.clone());
                        for (key, value) in &run_context {
                            request
                                .metadata
                                .entry(key.clone())
                                .or_insert_with(|| value.clone());
                        }
                        if !agent_session_permission_granted(&store, &request, session_id)
                            .map_err(|error| error.to_string())?
                        {
                            store
                                .save_permission_request(request.clone(), current_time_millis())
                                .map_err(|error| error.to_string())?;
                            append_event(
                                &mut store,
                                &runtime.task_id,
                                EventKind::PermissionRequested,
                                format!("Agent permission requested for {}", request.action),
                                metadata_with_context(
                                    [
                                        ("permission_id".to_string(), request.id.0),
                                        ("tool_call_id".to_string(), invocation.id.0),
                                        ("tool".to_string(), request.action),
                                        (
                                            "risk".to_string(),
                                            permission_risk_label(&request.risk).to_string(),
                                        ),
                                        ("scope".to_string(), request.scope),
                                        ("agent_prompt".to_string(), prompt.clone()),
                                    ]
                                    .into_iter()
                                    .collect(),
                                    &run_context,
                                ),
                            )
                            .map_err(|error| error.to_string())?;
                            waiting_for_permission = true;
                            continue;
                        }
                        append_event(
                            &mut store,
                            &runtime.task_id,
                            EventKind::PermissionResolved,
                            format!("Session permission reused for {}", request.action),
                            metadata_with_context(
                                [
                                    ("decision".to_string(), "allow_for_session".to_string()),
                                    ("tool_call_id".to_string(), invocation.id.0.clone()),
                                    ("tool".to_string(), request.action),
                                    ("scope".to_string(), request.scope),
                                ]
                                .into_iter()
                                .collect(),
                                &run_context,
                            ),
                        )
                        .map_err(|error| error.to_string())?;
                    }

                    let tool_name = invocation.tool_name.clone();
                    drop(store);
                    let result = execute_agent_tool_invocation(
                        state,
                        invocation,
                        workspace_root,
                        &run_context,
                    )?;
                    let observation = observation_from_tool_result(
                        &tool_name,
                        tool_outcome_label(&result.status),
                        &result.output,
                    );
                    record_tool_outcome(
                        &mut runtime,
                        &call.tool_name,
                        &call.input,
                        &result.status,
                    );
                    let previous_message_count = runtime.messages.len();
                    append_tool_observation(
                        &mut runtime,
                        call.call_id.clone(),
                        &observation,
                    );
                    let mut store = state
                        .store
                        .lock()
                        .map_err(|error| format!("store lock poisoned: {error}"))?;
                    persist_new_runtime_messages(
                        &mut store,
                        &runtime.task_id,
                        &runtime.messages,
                        previous_message_count,
                        &run_context,
                    )
                    .map_err(|error| error.to_string())?;
                }
                if waiting_for_permission {
                    let mut store = state
                        .store
                        .lock()
                        .map_err(|error| format!("store lock poisoned: {error}"))?;
                    append_event(
                        &mut store,
                        &runtime.task_id,
                        EventKind::TaskStatusChanged,
                        "Agent task waiting for permission",
                        run_context.clone(),
                    )
                    .map_err(|error| error.to_string())?;
                    remember_suspended_agent_run(
                        state,
                        SuspendedAgentRun {
                            runtime: runtime.clone(),
                            prompt: prompt.clone(),
                            run_context: run_context.clone(),
                            workspace_root: workspace_root.to_path_buf(),
                            collaboration: collaboration.cloned(),
                        },
                    )?;
                    return agent_state_for_session(&store, None, session_id)
                        .map_err(|error| error.to_string());
                }
            }
        }
    }
}

fn remember_suspended_agent_run(
    state: &tauri::State<'_, AppState>,
    run: SuspendedAgentRun,
) -> Result<(), String> {
    let Some(session_id) = run.run_context.get("session_id").cloned() else {
        return Ok(());
    };
    state
        .suspended_agent_runs
        .lock()
        .map_err(|error| format!("suspended agent runs lock poisoned: {error}"))?
        .insert(session_id, run);
    Ok(())
}

fn take_suspended_agent_run(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<Option<SuspendedAgentRun>, String> {
    Ok(state
        .suspended_agent_runs
        .lock()
        .map_err(|error| format!("suspended agent runs lock poisoned: {error}"))?
        .remove(session_id))
}

fn clear_suspended_agent_run(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<(), String> {
    let _ = take_suspended_agent_run(state, session_id)?;
    Ok(())
}

fn clear_suspended_agent_run_for_context(
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
) -> Result<(), String> {
    if let Some(session_id) = run_context.get("session_id") {
        clear_suspended_agent_run(state, session_id)?;
    }
    Ok(())
}

fn append_observations_to_suspended_run(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
    observations: &[ResolvedToolObservation],
) -> Result<(), String> {
    if session_id.is_empty() || observations.is_empty() {
        return Ok(());
    }
    let Some(mut suspended) = take_suspended_agent_run(state, session_id)? else {
        return Ok(());
    };
    for resolved in observations {
        record_tool_outcome(
            &mut suspended.runtime,
            &resolved.tool_name,
            &resolved.input_json,
            &resolved.status,
        );
        append_tool_observation(
            &mut suspended.runtime,
            resolved.call_id.clone(),
            &resolved.observation,
        );
    }
    remember_suspended_agent_run(state, suspended)
}

#[cfg(test)]
fn agent_state(
    store: &SqliteStore,
    last_error: Option<String>,
) -> Result<AgentState, StorageError> {
    agent_state_for_session(store, last_error, None)
}

fn agent_state_for_session(
    store: &SqliteStore,
    last_error: Option<String>,
    session_id: Option<&str>,
) -> Result<AgentState, StorageError> {
    let task_id = phase16_task_id();
    let events = store.list_by_task(&task_id)?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let thread_events = session_id
        .map(|session_id| agent_session_events(&events, session_id))
        .unwrap_or_else(|| active_events.clone());
    let run_context = agent_run_context_from_events(&active_events);
    let (active_start_ts, active_end_ts) = agent_run_time_bounds(&events, &active_events);
    let active_run_id = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_run_id"))
        .map(String::as_str);
    let audits = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id)
        .filter(|audit| {
            if let Some(session_id) = session_id {
                if audit.request.metadata.get("session_id").map(String::as_str)
                    != Some(session_id)
                {
                    return false;
                }
            }
            if let Some(active_run_id) = active_run_id {
                return audit
                    .request
                    .metadata
                    .get("agent_run_id")
                    .map(String::as_str)
                    == Some(active_run_id);
            }
            active_start_ts.is_some_and(|timestamp| {
                audit.requested_at_ms >= timestamp
                    && active_end_ts
                        .map(|end| audit.requested_at_ms < end)
                        .unwrap_or(true)
            })
        })
        .collect::<Vec<_>>();
    let timeline = thread_events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &audits))
        .collect::<Vec<_>>();
    let messages = thread_events
        .iter()
        .filter_map(message_view_from_event)
        .collect::<Vec<_>>();
    let mut pending_approvals = audits
        .iter()
        .cloned()
        .filter(|audit| audit.resolution.is_none())
        .filter_map(tool_approval_from_audit)
        .collect::<Vec<_>>();
    pending_approvals.reverse();
    let latest_answer = thread_events
        .iter()
        .rev()
        .filter(|event| matches!(event.kind, EventKind::MessageAdded))
        .find(|event| {
            event
                .metadata
                .get("role")
                .map(|role| role == "assistant")
                .unwrap_or(false)
        })
        .and_then(|event| event.metadata.get("content").cloned());
    let status = agent_status_from_events(&active_events, !pending_approvals.is_empty(), last_error.as_ref());
    if matches!(status.as_str(), "cancelled" | "completed" | "failed") {
        pending_approvals.clear();
    }
    let transcript_messages = agent_transcript_from_active_events(&thread_events).len();
    let context_window_tokens = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("context_window_tokens"))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(128_000)
        .max(1);
    let reported_context_tokens = active_events.iter().rev().find_map(|event| {
        (event.kind == EventKind::ModelRequestFinished
            && event.summary == "Agent model turn finished")
            .then(|| event.metadata.get("prompt_tokens")?.parse::<u64>().ok())
            .flatten()
    });
    let context_usage_estimated = reported_context_tokens.is_none();
    let context_tokens_used = reported_context_tokens.unwrap_or_else(|| {
        estimate_context_tokens(
            thread_events
                .iter()
                .filter_map(message_from_event)
                .collect::<Vec<_>>()
                .as_slice(),
        )
    });
    let context_remaining_percent =
        (context_window_tokens.saturating_sub(context_tokens_used) as f64
            / context_window_tokens as f64
            * 100.0)
            .clamp(0.0, 100.0);
    let turn_count = active_events
        .iter()
        .filter(|event| {
            matches!(event.kind, EventKind::ModelRequestFinished)
                && event.summary == "Agent model turn finished"
        })
        .count();
    let can_cancel = matches!(status.as_str(), "running" | "waiting_for_permission");
    let can_retry = matches!(status.as_str(), "completed" | "failed" | "cancelled")
        && latest_agent_prompt_from_active_events(&active_events).is_some();

    Ok(AgentState {
        task_id: task_id.0,
        project_id: run_context.project_id,
        project_name: run_context.project_name,
        session_id: run_context.session_id,
        session_name: run_context.session_name,
        status,
        turn_count,
        max_turns: AgentRuntimeConfig::default().max_turns,
        transcript_messages,
        context_tokens_used,
        context_window_tokens,
        context_remaining_percent,
        context_usage_estimated,
        can_cancel,
        can_retry,
        timeline,
        messages,
        pending_approvals,
        latest_answer,
        last_error,
    })
}

fn estimate_context_tokens(messages: &[Message]) -> u64 {
    let text_tokens = messages
        .iter()
        .map(|message| (message.content.chars().count() as u64 + 3) / 4)
        .sum::<u64>();
    768_u64
        .saturating_add(text_tokens)
        .saturating_add(messages.len() as u64 * 6)
}

fn agent_state_with_error_in_context(
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    message: impl Into<String>,
) -> Result<AgentState, String> {
    let message = message.into();
    let session_id = run_context.get("session_id").map(String::as_str);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::Error,
        "Agent task failed",
        metadata_with_context(
            [("error".to_string(), message.clone())]
                .into_iter()
                .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;

    agent_state_for_session(&store, Some(message), session_id).map_err(|error| error.to_string())
}

fn agent_task_is_cancelled(
    store: &SqliteStore,
    session_id: Option<&str>,
) -> Result<bool, StorageError> {
    let events = store.list_by_task(&phase16_task_id())?;
    Ok(active_agent_events_for_session(&events, session_id)
        .iter()
        .rev()
        .find(|event| matches!(event.kind, EventKind::TaskStatusChanged))
        .map(|event| event.summary == "Agent task cancelled")
        .unwrap_or(false))
}

fn agent_session_permission_granted(
    store: &SqliteStore,
    request: &PermissionRequest,
    session_id: Option<&str>,
) -> Result<bool, StorageError> {
    if matches!(&request.risk, PermissionRisk::Destructive) {
        return Ok(false);
    }
    let Some(session_id) = session_id else {
        return Ok(false);
    };
    Ok(store.list_permission_audits()?.iter().any(|audit| {
        audit.request.task_id == phase16_task_id()
            && audit.request.action == request.action
            && audit.request.scope == request.scope
            && audit.request.metadata.get("session_id").map(String::as_str) == Some(session_id)
            && audit
                .resolution
                .as_ref()
                .is_some_and(|resolution| {
                    matches!(&resolution.decision, PermissionDecision::AllowForSession)
                })
    }))
}

fn pending_agent_permissions_for_run(
    store: &SqliteStore,
    session_id: Option<&str>,
    agent_run_id: Option<&str>,
) -> Result<Vec<PermissionRequest>, StorageError> {
    let mut requests = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == phase16_task_id())
        .filter(|audit| audit.resolution.is_none())
        .filter(|audit| {
            session_id.is_none_or(|session_id| {
                audit.request.metadata.get("session_id").map(String::as_str) == Some(session_id)
            })
        })
        .filter(|audit| {
            agent_run_id.is_none_or(|agent_run_id| {
                audit
                    .request
                    .metadata
                    .get("agent_run_id")
                    .map(String::as_str)
                    == Some(agent_run_id)
            })
        })
        .map(|audit| audit.request)
        .collect::<Vec<_>>();
    requests.reverse();
    Ok(requests)
}

fn agent_status_from_events(
    events: &[Event],
    has_pending_approval: bool,
    last_error: Option<&String>,
) -> String {
    if last_error.is_some()
        || events
            .iter()
            .any(|event| matches!(event.kind, EventKind::Error))
    {
        return "failed".to_string();
    }

    if let Some(status_event) = events
        .iter()
        .rev()
        .find(|event| matches!(event.kind, EventKind::TaskStatusChanged))
    {
        match status_event.summary.as_str() {
            "Agent task cancelled" => return "cancelled".to_string(),
            "Agent task completed" => return "completed".to_string(),
            "Agent task waiting for permission" => return "waiting_for_permission".to_string(),
            _ => {}
        }
    }

    if has_pending_approval {
        "waiting_for_permission".to_string()
    } else if events.is_empty() {
        "idle".to_string()
    } else {
        "running".to_string()
    }
}

fn latest_agent_prompt_from_active_events(active_events: &[Event]) -> Option<String> {
    active_events
        .iter()
        .find(|event| {
            matches!(event.kind, EventKind::MessageAdded)
                && event
                    .metadata
                    .get("role")
                    .map(|role| role == "user")
                    .unwrap_or(false)
        })
        .and_then(|event| event.metadata.get("content").cloned())
        .or_else(|| {
            active_events
                .iter()
                .find(|event| {
                    matches!(event.kind, EventKind::TaskStatusChanged)
                        && is_agent_run_start_event(event)
                })
                .and_then(|event| event.metadata.get("prompt").cloned())
        })
}

fn agent_transcript_from_active_events(events: &[Event]) -> Vec<Message> {
    events.iter().filter_map(message_from_event).collect()
}

fn agent_trace_state_for_session(
    store: &SqliteStore,
    export_path: Option<PathBuf>,
    last_error: Option<String>,
    session_id: Option<&str>,
) -> Result<AgentTraceState, StorageError> {
    let task_id = phase16_task_id();
    let events = store.list_by_task(&task_id)?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let run_context = agent_run_context_from_events(&active_events);
    let (active_start_ts, active_end_ts) = agent_run_time_bounds(&events, &active_events);
    let active_run_id = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_run_id"))
        .map(String::as_str);
    let audits = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id)
        .filter(|audit| {
            if let Some(session_id) = session_id {
                if audit.request.metadata.get("session_id").map(String::as_str)
                    != Some(session_id)
                {
                    return false;
                }
            }
            if let Some(active_run_id) = active_run_id {
                return audit
                    .request
                    .metadata
                    .get("agent_run_id")
                    .map(String::as_str)
                    == Some(active_run_id);
            }
            active_start_ts.is_some_and(|timestamp| {
                audit.requested_at_ms >= timestamp
                    && active_end_ts
                        .map(|end| audit.requested_at_ms < end)
                        .unwrap_or(true)
            })
        })
        .collect::<Vec<_>>();
    let has_pending_approval = audits.iter().any(|audit| audit.resolution.is_none());
    let status = agent_status_from_events(&active_events, has_pending_approval, last_error.as_ref());
    let turns = agent_trace_turns_from_events(&active_events, &audits);
    let step_count = turns.iter().map(|turn| turn.steps.len()).sum::<usize>();
    let tool_call_count = turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .filter(|step| step.tool_call_id.is_some())
        .count();
    let permission_wait_count = turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .filter(|step| step.permission_id.is_some())
        .count();
    let error_count = turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .filter(|step| step.kind == "error")
        .count();
    let started_at_ms = active_events
        .first()
        .map(|event| event.timestamp_ms)
        .unwrap_or_default();
    let finished_at_ms = if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
        active_events.last().map(|event| event.timestamp_ms)
    } else {
        None
    };
    let duration_ms = finished_at_ms.map(|finished| finished.saturating_sub(started_at_ms));
    let run_id = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .map(|event| event.id.0.clone())
        .unwrap_or_else(|| "no-run".to_string());
    let start_sequence = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .map(|event| event.sequence)
        .unwrap_or_default();

    Ok(AgentTraceState {
        task_id: task_id.0.clone(),
        trace_id: format!("{}-{start_sequence}", task_id.0),
        run_id,
        project_id: run_context.project_id,
        project_name: run_context.project_name,
        session_id: run_context.session_id,
        session_name: run_context.session_name,
        status,
        started_at_ms,
        finished_at_ms,
        duration_ms,
        turn_count: turns.iter().filter(|turn| turn.index > 0).count(),
        step_count,
        tool_call_count,
        permission_wait_count,
        error_count,
        export_path: export_path.map(|path| path.display().to_string()),
        turns,
        last_error,
    })
}

fn agent_trace_turns_from_events(
    events: &[Event],
    audits: &[PermissionAuditRecord],
) -> Vec<AgentTraceTurnView> {
    let mut starts = TraceStarts::default();
    let mut turns: BTreeMap<usize, Vec<AgentTraceStepView>> = BTreeMap::new();
    let mut current_turn = 0usize;

    for event in events {
        if matches!(event.kind, EventKind::ModelRequestStarted) {
            current_turn = event
                .metadata
                .get("turn")
                .and_then(|value| value.parse::<usize>().ok())
                .map(|turn| turn + 1)
                .unwrap_or_else(|| current_turn.max(1));
        }

        let step = agent_trace_step_from_event(event, current_turn, audits, &starts);
        starts.record(event);
        turns.entry(step.turn_index).or_default().push(step);
    }

    turns
        .into_iter()
        .map(|(index, steps)| {
            let started_at_ms = steps
                .iter()
                .map(|step| step.started_at_ms)
                .min()
                .unwrap_or_default();
            let finished_at_ms = trace_turn_finished_at(&steps);
            let duration_ms = finished_at_ms.map(|finished| finished.saturating_sub(started_at_ms));
            let status = trace_turn_status(&steps);
            AgentTraceTurnView {
                index,
                label: if index == 0 {
                    "Run setup".to_string()
                } else {
                    format!("Turn {index}")
                },
                status,
                started_at_ms,
                finished_at_ms,
                duration_ms,
                steps,
            }
        })
        .collect()
}

#[derive(Default)]
struct TraceStarts {
    model_requests: BTreeMap<String, u64>,
    tool_calls: BTreeMap<String, u64>,
    permissions: BTreeMap<String, u64>,
}

impl TraceStarts {
    fn record(&mut self, event: &Event) {
        match event.kind {
            EventKind::ModelRequestStarted => {
                if let Some(request_id) = event.metadata.get("request_id") {
                    self.model_requests.insert(request_id.clone(), event.timestamp_ms);
                }
            }
            EventKind::ToolCallStarted => {
                if let Some(tool_call_id) = event.metadata.get("tool_call_id") {
                    self.tool_calls.insert(tool_call_id.clone(), event.timestamp_ms);
                }
            }
            EventKind::PermissionRequested => {
                if let Some(permission_id) = event.metadata.get("permission_id") {
                    self.permissions.insert(permission_id.clone(), event.timestamp_ms);
                }
            }
            _ => {}
        }
    }
}

fn agent_trace_step_from_event(
    event: &Event,
    turn_index: usize,
    audits: &[PermissionAuditRecord],
    starts: &TraceStarts,
) -> AgentTraceStepView {
    let request_id = event.metadata.get("request_id").cloned();
    let tool_call_id = event.metadata.get("tool_call_id").cloned();
    let permission_id = event.metadata.get("permission_id").cloned();
    let started_at_ms = trace_step_started_at(event, &request_id, &tool_call_id, &permission_id, starts);
    let finished_at_ms = trace_step_finished_at(event);
    let latency_ms = event
        .metadata
        .get("latency_ms")
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| finished_at_ms.map(|finished| finished.saturating_sub(started_at_ms)));
    let timeline = timeline_entry(event.clone(), audits);

    AgentTraceStepView {
        id: event.id.0.clone(),
        parent_id: trace_parent_id(turn_index, &tool_call_id, &permission_id),
        turn_index,
        sequence: event.sequence,
        kind: trace_kind_label(&event.kind).to_string(),
        label: redact_sensitive_text(&event.summary),
        status: trace_step_status(event, audits),
        started_at_ms,
        finished_at_ms,
        latency_ms,
        model: event.metadata.get("model").cloned(),
        tool_name: event.metadata.get("tool").cloned(),
        request_id,
        tool_call_id,
        permission_id,
        input_preview: trace_input_preview(event),
        output_preview: trace_output_preview(event),
        artifact_path: trace_artifact_path(event),
        detail: timeline.detail,
        metadata: redact_metadata(&event.metadata),
    }
}

fn trace_step_started_at(
    event: &Event,
    request_id: &Option<String>,
    tool_call_id: &Option<String>,
    permission_id: &Option<String>,
    starts: &TraceStarts,
) -> u64 {
    match event.kind {
        EventKind::ModelRequestFinished => request_id
            .as_ref()
            .and_then(|id| starts.model_requests.get(id).copied())
            .unwrap_or(event.timestamp_ms),
        EventKind::ToolCallFinished => tool_call_id
            .as_ref()
            .and_then(|id| starts.tool_calls.get(id).copied())
            .unwrap_or(event.timestamp_ms),
        EventKind::PermissionResolved => permission_id
            .as_ref()
            .and_then(|id| starts.permissions.get(id).copied())
            .unwrap_or(event.timestamp_ms),
        _ => event.timestamp_ms,
    }
}

fn trace_step_finished_at(event: &Event) -> Option<u64> {
    match event.kind {
        EventKind::ModelRequestFinished
        | EventKind::ToolCallFinished
        | EventKind::PermissionResolved
        | EventKind::Error => Some(event.timestamp_ms),
        _ => None,
    }
}

fn trace_parent_id(
    turn_index: usize,
    tool_call_id: &Option<String>,
    permission_id: &Option<String>,
) -> Option<String> {
    permission_id
        .as_ref()
        .map(|id| format!("permission:{id}"))
        .or_else(|| tool_call_id.as_ref().map(|id| format!("tool:{id}")))
        .or_else(|| (turn_index > 0).then(|| format!("turn:{turn_index}")))
}

fn trace_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished => "model",
        EventKind::ToolCallProposed | EventKind::ToolCallStarted | EventKind::ToolCallFinished => {
            "tool"
        }
        EventKind::PermissionRequested | EventKind::PermissionResolved => "permission",
        EventKind::Error => "error",
        EventKind::MessageAdded => "message",
        EventKind::RetrievalPerformed => "retrieval",
        _ => "status",
    }
}

fn trace_step_status(event: &Event, audits: &[PermissionAuditRecord]) -> String {
    match event.kind {
        EventKind::Error => "failed".to_string(),
        EventKind::ModelRequestStarted | EventKind::ToolCallStarted => "running".to_string(),
        EventKind::PermissionRequested => {
            if event
                .metadata
                .get("permission_id")
                .is_some_and(|id| audits.iter().any(|audit| audit.request.id.0 == *id && audit.resolution.is_none()))
            {
                "pending".to_string()
            } else {
                "done".to_string()
            }
        }
        EventKind::PermissionResolved => event
            .metadata
            .get("decision")
            .cloned()
            .unwrap_or_else(|| "done".to_string()),
        EventKind::ToolCallFinished => event
            .metadata
            .get("status")
            .cloned()
            .unwrap_or_else(|| "done".to_string()),
        EventKind::TaskStatusChanged => match event.summary.as_str() {
            "Agent task waiting for permission" => "waiting".to_string(),
            "Agent task completed" => "completed".to_string(),
            "Agent task cancelled" => "cancelled".to_string(),
            _ => "done".to_string(),
        },
        _ => "done".to_string(),
    }
}

fn trace_input_preview(event: &Event) -> Option<String> {
    event
        .metadata
        .get("input_preview")
        .or_else(|| event.metadata.get("tool_input"))
        .or_else(|| event.metadata.get("prompt"))
        .or_else(|| event.metadata.get("content").filter(|_| {
            event.metadata.get("role").map(String::as_str) == Some("user")
        }))
        .map(|value| truncate_for_timeline(&redact_sensitive_text(value)))
}

fn trace_output_preview(event: &Event) -> Option<String> {
    event
        .metadata
        .get("output")
        .or_else(|| event.metadata.get("error"))
        .or_else(|| event.metadata.get("content").filter(|_| {
            event.metadata.get("role").map(String::as_str) != Some("user")
        }))
        .map(|value| truncate_for_timeline(&redact_sensitive_text(value)))
}

fn trace_artifact_path(event: &Event) -> Option<String> {
    event
        .metadata
        .get("result_artifact_path")
        .or_else(|| event.metadata.get("result_text_path"))
        .or_else(|| {
            (event.metadata.get("tool").map(String::as_str) == Some("file.write"))
                .then(|| event.metadata.get("result_path"))
                .flatten()
        })
        .or_else(|| event.metadata.get("context_checkpoint_path"))
        .or_else(|| event.metadata.get("lancedb_export_path"))
        .cloned()
}

fn trace_turn_finished_at(steps: &[AgentTraceStepView]) -> Option<u64> {
    if steps.iter().any(|step| matches!(step.status.as_str(), "running" | "pending" | "waiting")) {
        None
    } else {
        steps
            .iter()
            .filter_map(|step| step.finished_at_ms.or(Some(step.started_at_ms)))
            .max()
    }
}

fn trace_turn_status(steps: &[AgentTraceStepView]) -> String {
    if steps.iter().any(|step| step.status == "failed") {
        "failed".to_string()
    } else if steps.iter().any(|step| step.status == "pending" || step.status == "waiting") {
        "waiting".to_string()
    } else if steps.iter().any(|step| step.status == "running") {
        "running".to_string()
    } else if steps.iter().any(|step| step.status == "cancelled") {
        "cancelled".to_string()
    } else if steps.iter().any(|step| step.status == "completed") {
        "completed".to_string()
    } else {
        "done".to_string()
    }
}

#[cfg(test)]
fn active_agent_events(events: &[Event]) -> Vec<Event> {
    active_agent_events_for_session(events, None)
}

fn active_agent_events_for_session(events: &[Event], session_id: Option<&str>) -> Vec<Event> {
    let start_event = events
        .iter()
        .rev()
        .find(|event| {
            is_agent_run_start_event(event)
                && session_id
                    .map(|session_id| {
                        event.metadata.get("session_id").map(String::as_str) == Some(session_id)
                    })
                    .unwrap_or(true)
        });
    if session_id.is_some() && start_event.is_none() {
        return Vec::new();
    }
    let start_sequence = start_event.map(|event| event.sequence);
    let run_session_id = start_event
        .and_then(|event| event.metadata.get("session_id"))
        .map(String::as_str)
        .or(session_id);
    let end_sequence = start_sequence.and_then(|start_sequence| {
        events
            .iter()
            .find(|event| {
                event.sequence > start_sequence
                    && is_agent_run_start_event(event)
                    && run_session_id
                        .map(|session_id| {
                            event.metadata.get("session_id").map(String::as_str)
                                == Some(session_id)
                        })
                        .unwrap_or(true)
            })
            .map(|event| event.sequence)
    });
    events
        .iter()
        .filter(|event| {
            start_sequence
                .map(|sequence| {
                    event.sequence >= sequence
                        && end_sequence
                            .map(|end_sequence| event.sequence < end_sequence)
                            .unwrap_or(true)
                        && session_id
                            .map(|session_id| {
                                event
                                    .metadata
                                    .get("session_id")
                                    .map(|value| value == session_id)
                                    .unwrap_or(true)
                            })
                            .unwrap_or(true)
                })
                .unwrap_or(true)
        })
        .cloned()
        .map(redact_event)
        .collect()
}

fn agent_session_events(events: &[Event], session_id: &str) -> Vec<Event> {
    let mut current_session_id: Option<&str> = None;
    events
        .iter()
        .filter_map(|event| {
            if is_agent_run_start_event(event) {
                current_session_id = event.metadata.get("session_id").map(String::as_str);
            }
            let event_session_id = event.metadata.get("session_id").map(String::as_str);
            (event_session_id
                .map(|event_session_id| event_session_id == session_id)
                .unwrap_or(current_session_id == Some(session_id)))
            .then(|| redact_event(event.clone()))
        })
        .collect()
}

fn agent_run_time_bounds(events: &[Event], active_events: &[Event]) -> (Option<u64>, Option<u64>) {
    let start_event = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .cloned();
    let start_sequence = start_event.as_ref().map(|event| event.sequence);
    let session_id = start_event
        .as_ref()
        .and_then(|event| event.metadata.get("session_id"))
        .map(String::as_str);
    let start_timestamp = start_sequence.and_then(|start_sequence| {
        active_events
            .iter()
            .find(|event| event.sequence == start_sequence)
            .map(|event| event.timestamp_ms)
    });
    let end_timestamp = start_sequence.and_then(|start_sequence| {
        events
            .iter()
            .find(|event| {
                event.sequence > start_sequence
                    && is_agent_run_start_event(event)
                    && session_id
                        .map(|session_id| {
                            event.metadata.get("session_id").map(String::as_str)
                                == Some(session_id)
                        })
                        .unwrap_or(true)
            })
            .map(|event| event.timestamp_ms)
    });
    (start_timestamp, end_timestamp)
}

fn is_agent_run_start_event(event: &Event) -> bool {
    matches!(event.kind, EventKind::TaskStatusChanged)
        && matches!(
            event.summary.as_str(),
            "Agent task started" | "Agent task retry started"
        )
}

fn phase8_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<Phase8State, String> {
    let message = message.into();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase8_task_id(),
        EventKind::Error,
        "Browser tool failed",
        [("error".to_string(), message.clone())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase8_state(&store, Some(message)).map_err(|error| error.to_string())
}

fn phase6_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<Phase6State, String> {
    let message = message.into();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase6_task_id(),
        EventKind::Error,
        "Orchestration failed",
        [("error".to_string(), message.clone())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase6_state(&store, Some(message)).map_err(|error| error.to_string())
}

fn phase5_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<Phase5State, String> {
    let message = message.into();
    let root = active_workspace_root(state)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase5_task_id(),
        EventKind::Error,
        "Tool call failed",
        [("error".to_string(), message.clone())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase5_state(&store, Some(message), &root).map_err(|error| error.to_string())
}

fn execute_tool_invocation(
    store: &mut SqliteStore,
    invocation: ToolInvocation,
    workspace_root: &Path,
) -> Result<(), StorageError> {
    execute_tool_invocation_with_result(store, invocation, workspace_root, None).map(|_| ())
}

fn execute_agent_tool_invocation(
    state: &tauri::State<'_, AppState>,
    invocation: ToolInvocation,
    workspace_root: &Path,
    run_context: &Metadata,
) -> Result<ToolResult, String> {
    let task_id = invocation.task_id.clone();
    let tool_call_id = invocation.id.0.clone();
    let tool_name = invocation.tool_name.clone();
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &task_id,
            EventKind::ToolCallStarted,
            format!("Tool call started: {tool_name}"),
            metadata_with_context(
                [
                    ("tool_call_id".to_string(), tool_call_id.clone()),
                    ("tool".to_string(), tool_name.clone()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }

    let registry = tool_registry_for_state(state, workspace_root)?;
    let mut result = match registry.get(&tool_name) {
        Some(tool) => match tool.execute(invocation) {
            Ok(result) => result,
            Err(error) => ToolResult::failed(
                agent_core::ToolCallId(tool_call_id.clone()),
                error.message,
            ),
        },
        None => ToolResult::failed(
            agent_core::ToolCallId(tool_call_id.clone()),
            "unknown tool",
        ),
    };
    materialize_tool_result_artifacts(&mut result, workspace_root)?;

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_tool_finished_event(
        &mut store,
        &task_id,
        &tool_call_id,
        &tool_name,
        tool_outcome_label(&result.status),
        &result.output,
        result.metadata.clone(),
        Some(run_context),
    )
    .map_err(|error| error.to_string())?;
    Ok(result)
}

fn materialize_tool_result_artifacts(
    result: &mut ToolResult,
    workspace_root: &Path,
) -> Result<(), String> {
    let output_dir = workspace_root.join(".cindx").join("artifacts");
    let mut image_index = 0usize;
    for content in &result.content {
        let ToolContent::Image { mime_type, data } = content else {
            continue;
        };
        fs::create_dir_all(&output_dir)
            .map_err(|error| format!("failed to create tool artifact directory: {error}"))?;
        let extension = match mime_type.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        let filename = format!("{}-{image_index}.{extension}", result.invocation_id.0);
        let path = output_dir.join(&filename);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|error| format!("invalid tool image data: {error}"))?;
        fs::write(&path, bytes)
            .map_err(|error| format!("failed to write tool image artifact: {error}"))?;
        result.artifacts.push(ToolArtifact {
            path: path.display().to_string(),
            mime_type: Some(mime_type.clone()),
            title: Some("MCP image output".to_string()),
        });
        image_index += 1;
    }
    if result.artifacts.is_empty() {
        if let Some(path) = result.metadata.get("artifact_path").cloned() {
            result.artifacts.push(ToolArtifact {
                path,
                mime_type: None,
                title: None,
            });
        }
    }
    if let Some(structured) = &result.structured_output_json {
        result
            .metadata
            .insert("structured_output".to_string(), structured.clone());
    }
    if !result.artifacts.is_empty() {
        result.metadata.insert(
            "artifact_count".to_string(),
            result.artifacts.len().to_string(),
        );
        result.metadata.insert(
            "artifact_path".to_string(),
            result.artifacts[0].path.clone(),
        );
    }
    Ok(())
}

fn execute_tool_invocation_with_result(
    store: &mut SqliteStore,
    invocation: ToolInvocation,
    workspace_root: &Path,
    run_context: Option<&Metadata>,
) -> Result<ToolResult, StorageError> {
    let metadata = [
        ("tool_call_id".to_string(), invocation.id.0.clone()),
        ("tool".to_string(), invocation.tool_name.clone()),
    ]
    .into_iter()
    .collect();
    let metadata = match run_context {
        Some(context) => metadata_with_context(metadata, context),
        None => metadata,
    };
    append_event(
        store,
        &invocation.task_id,
        EventKind::ToolCallStarted,
        format!("Tool call started: {}", invocation.tool_name),
        metadata,
    )?;

    let task_id = invocation.task_id.clone();
    let tool_call_id = invocation.id.0.clone();
    let tool_name = invocation.tool_name.clone();
    let registry = ToolRegistry::with_workspace_tools(workspace_root.to_path_buf());
    let Some(tool) = registry.get(&invocation.tool_name) else {
        let result = ToolResult::failed(invocation.id, "unknown tool");
        append_tool_finished_event(
            store,
            &task_id,
            &tool_call_id,
            &tool_name,
            "failed",
            "unknown tool",
            Metadata::new(),
            run_context,
        )?;
        return Ok(result);
    };

    let result = match tool.execute(invocation) {
        Ok(result) => {
            append_tool_finished_event(
                store,
                &task_id,
                &tool_call_id,
                &tool_name,
                tool_outcome_label(&result.status),
                &result.output,
                result.metadata.clone(),
                run_context,
            )?;
            result
        }
        Err(error) => {
            append_tool_finished_event(
                store,
                &task_id,
                &tool_call_id,
                &tool_name,
                "failed",
                &error.message,
                Metadata::new(),
                run_context,
            )?;
            ToolResult::failed(agent_core::ToolCallId(tool_call_id), error.message)
        }
    };

    Ok(result)
}

fn append_tool_proposed_event(
    store: &mut SqliteStore,
    invocation: &ToolInvocation,
    run_context: Option<&Metadata>,
) -> Result<(), StorageError> {
    let metadata = [
        ("tool_call_id".to_string(), invocation.id.0.clone()),
        ("tool".to_string(), invocation.tool_name.clone()),
        (
            "input_preview".to_string(),
            truncate_for_timeline(&invocation.input_json),
        ),
        (
            "input_length".to_string(),
            invocation.input_json.len().to_string(),
        ),
    ]
    .into_iter()
    .collect();
    let metadata = match run_context {
        Some(context) => metadata_with_context(metadata, context),
        None => metadata,
    };
    append_event(
        store,
        &invocation.task_id,
        EventKind::ToolCallProposed,
        format!("Tool call proposed: {}", invocation.tool_name),
        metadata,
    )
}

fn append_tool_finished_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    tool_call_id: &str,
    tool_name: &str,
    status: &str,
    output: &str,
    result_metadata: Metadata,
    run_context: Option<&Metadata>,
) -> Result<(), StorageError> {
    let mut metadata = Metadata::new();
    metadata.insert("tool_call_id".to_string(), tool_call_id.to_string());
    metadata.insert("tool".to_string(), tool_name.to_string());
    metadata.insert("status".to_string(), status.to_string());
    metadata.insert("output".to_string(), output.to_string());
    metadata.insert("output_length".to_string(), output.len().to_string());
    for (key, value) in result_metadata {
        metadata.insert(format!("result_{key}"), value);
    }
    if let Some(context) = run_context {
        metadata = metadata_with_context(metadata, context);
    }

    append_event(
        store,
        task_id,
        EventKind::ToolCallFinished,
        format!("Tool call finished: {tool_name}"),
        metadata,
    )
}

fn phase4_state_with_error(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    message: &str,
) -> Result<Phase4State, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase4_state(&store, config, Some(message.to_string())).map_err(|error| error.to_string())
}

fn record_phase4_error(
    state: &tauri::State<'_, AppState>,
    message: &str,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase4_task_id(),
        EventKind::Error,
        "Model request failed",
        [("error".to_string(), message.to_string())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())
}

fn append_message_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    role: MessageRole,
    content: &str,
) -> Result<(), StorageError> {
    append_message_event_with_metadata(store, task_id, role, content, Metadata::new())
}

fn append_tool_message_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    tool_call_id: &str,
    tool_name: &str,
    status: &str,
    content: &str,
    run_context: Option<&Metadata>,
) -> Result<(), StorageError> {
    let metadata = [
        ("kind".to_string(), "tool_observation".to_string()),
        ("tool_call_id".to_string(), tool_call_id.to_string()),
        ("tool".to_string(), tool_name.to_string()),
        ("status".to_string(), status.to_string()),
    ]
    .into_iter()
    .collect();
    let metadata = match run_context {
        Some(context) => metadata_with_context(metadata, context),
        None => metadata,
    };
    append_message_event_with_metadata(
        store,
        task_id,
        MessageRole::Tool,
        content,
        metadata,
    )
}

fn persist_new_runtime_messages(
    store: &mut SqliteStore,
    task_id: &TaskId,
    messages: &[Message],
    previous_message_count: usize,
    run_context: &Metadata,
) -> Result<(), StorageError> {
    for message in messages.iter().skip(previous_message_count) {
        append_message_event_with_metadata(
            store,
            task_id,
            message.role.clone(),
            &message.content,
            metadata_with_context(message.metadata.clone(), run_context),
        )?;
    }

    Ok(())
}

fn append_message_event_with_metadata(
    store: &mut SqliteStore,
    task_id: &TaskId,
    role: MessageRole,
    content: &str,
    mut metadata: Metadata,
) -> Result<(), StorageError> {
    metadata.insert("role".to_string(), message_role_label(&role).to_string());
    metadata.insert("content".to_string(), content.to_string());
    metadata.insert("content_length".to_string(), content.len().to_string());

    append_event(
        store,
        task_id,
        EventKind::MessageAdded,
        format!("{} message", message_role_label(&role)),
        metadata,
    )
}

fn append_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    kind: EventKind,
    summary: impl Into<String>,
    metadata: Metadata,
) -> Result<(), StorageError> {
    let sequence = store.next_sequence(task_id)?;
    let metadata = redact_metadata(&metadata);

    store.append(Event {
        id: EventId(unique_id("event")),
        task_id: task_id.clone(),
        sequence,
        timestamp_ms: current_time_millis(),
        kind,
        summary: redact_sensitive_text(&summary.into()),
        metadata,
    })
}

fn redact_metadata(metadata: &Metadata) -> Metadata {
    metadata
        .iter()
        .map(|(key, value)| {
            let redacted = if is_sensitive_assignment_key(key) {
                "[REDACTED]".to_string()
            } else {
                redact_sensitive_text(value)
            };
            (key.clone(), redacted)
        })
        .collect()
}

fn redact_event(mut event: Event) -> Event {
    event.summary = redact_sensitive_text(&event.summary);
    event.metadata = redact_metadata(&event.metadata);
    event
}

fn redact_persisted_events(store: &mut SqliteStore) -> Result<usize, StorageError> {
    let mut updated = 0;
    for event in store.list_all_events()? {
        let redacted = redact_event(event.clone());
        if redacted.summary != event.summary || redacted.metadata != event.metadata {
            store.update_event_content(&redacted)?;
            updated += 1;
        }
    }
    Ok(updated)
}

fn redact_existing_text_artifact(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read text artifact: {error}"))?;
    let redacted = redact_sensitive_text(&text);
    if redacted == text {
        return Ok(false);
    }

    let mut options = fs::OpenOptions::new();
    options.truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("failed to open text artifact: {error}"))?;
    file.write_all(redacted.as_bytes())
        .map_err(|error| format!("failed to rewrite text artifact: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure text artifact: {error}"))?;
    Ok(true)
}

fn redact_sensitive_text(value: &str) -> String {
    value
        .split('\n')
        .map(redact_sensitive_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_sensitive_line(line: &str) -> String {
    for (index, character) in line.char_indices() {
        if matches!(character, '=' | ':') && is_sensitive_assignment_key(&line[..index]) {
            return format!("{}[REDACTED]", &line[..=index]);
        }
    }

    let lower = line.to_ascii_lowercase();
    if let Some(index) = lower.find("bearer ") {
        return format!("{}Bearer [REDACTED]", &line[..index]);
    }

    [
        ("github_pat_", 20_usize),
        ("ghp_", 16_usize),
        ("xoxb-", 16_usize),
        ("sk-", 16_usize),
        ("AKIA", 16_usize),
    ]
    .into_iter()
    .fold(line.to_string(), |text, (prefix, minimum_length)| {
        redact_prefixed_secret(&text, prefix, minimum_length)
    })
}

fn is_sensitive_assignment_key(value: &str) -> bool {
    let compact = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "apikey",
        "xapikey",
        "authorization",
        "proxyauthorization",
        "accesstoken",
        "refreshtoken",
        "authtoken",
        "clientsecret",
        "secretkey",
        "password",
    ]
    .iter()
    .any(|key| compact.ends_with(key))
}

fn redact_prefixed_secret(value: &str, prefix: &str, minimum_length: usize) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_start) = value[cursor..].find(prefix) {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let mut end = start + prefix.len();
        while end < value.len() {
            let byte = value.as_bytes()[end];
            if byte.is_ascii_whitespace()
                || matches!(byte, b'\'' | b'"' | b',' | b';' | b')' | b']' | b'}')
            {
                break;
            }
            end += 1;
        }
        if end.saturating_sub(start) >= minimum_length {
            output.push_str("[REDACTED]");
        } else {
            output.push_str(prefix);
        }
        cursor = end;
    }
    output.push_str(&value[cursor..]);
    output
}

fn timeline_entry(event: Event, audits: &[PermissionAuditRecord]) -> TimelineEntry {
    let permission_id = event.metadata.get("permission_id");
    let permission_is_pending = permission_id.is_some_and(|id| {
        audits
            .iter()
            .any(|audit| audit.request.id.0 == *id && audit.resolution.is_none())
    });
    let detail = match event.kind {
        EventKind::MessageAdded => event
            .metadata
            .get("content")
            .map(|content| {
                format!(
                    "{}: {}",
                    event
                        .metadata
                        .get("role")
                        .cloned()
                        .unwrap_or_else(|| "message".to_string()),
                    truncate_for_timeline(content)
                )
            })
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::ModelRequestStarted => event
            .metadata
            .get("model")
            .map(|model| format!("{} using {model}", event.summary))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::ModelRequestFinished => {
            let latency = event.metadata.get("latency_ms").cloned().unwrap_or_default();
            if latency.is_empty() {
                event.summary.clone()
            } else {
                format!("{} in {latency} ms", event.summary)
            }
        }
        EventKind::ToolCallProposed => event
            .metadata
            .get("input_preview")
            .map(|input| {
                format!(
                    "{} with {}",
                    event.summary,
                    input.replace('\n', " ")
                )
            })
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::ToolCallStarted => event
            .metadata
            .get("tool")
            .map(|tool| format!("Executing {tool}"))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::ToolCallFinished => {
            let status = event
                .metadata
                .get("status")
                .cloned()
                .unwrap_or_else(|| "done".to_string());
            let output = event
                .metadata
                .get("output")
                .map(|value| truncate_for_timeline(value))
                .unwrap_or_default();
            if output.is_empty() {
                format!("{}: {status}", event.summary)
            } else {
                format!("{}: {status}. {output}", event.summary)
            }
        }
        EventKind::PermissionRequested => event
            .metadata
            .get("scope")
            .map(|scope| format!("{} Scope: {scope}", event.summary))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::PermissionResolved => event
            .metadata
            .get("decision")
            .map(|decision| format!("{} with {decision}", event.summary))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::Error => event
            .metadata
            .get("error")
            .cloned()
            .unwrap_or_else(|| event.summary.clone()),
        _ => event.summary.clone(),
    };

    TimelineEntry {
        label: event_kind_label(&event.kind).to_string(),
        detail: redact_sensitive_text(&detail),
        kind: event_kind_ui_kind(&event.kind).to_string(),
        state: event_state(&event.kind, permission_is_pending).to_string(),
        timestamp_ms: event.timestamp_ms,
    }
}

fn permission_audit(record: PermissionAuditRecord) -> PermissionAudit {
    let resolution = record.resolution;
    PermissionAudit {
        id: record.request.id.0,
        risk: permission_risk_label(&record.request.risk).to_string(),
        action: redact_sensitive_text(&record.request.action),
        reason: redact_sensitive_text(&record.request.reason),
        scope: redact_sensitive_text(&record.request.scope),
        status: if resolution.is_some() {
            "resolved".to_string()
        } else {
            "pending".to_string()
        },
        decision: resolution
            .as_ref()
            .map(|resolution| permission_decision_label(&resolution.decision).to_string()),
        requested_at_ms: record.requested_at_ms,
        resolved_at_ms: resolution.map(|resolution| resolution.resolved_at_ms),
    }
}

fn message_view_from_event(event: &Event) -> Option<ChatMessageView> {
    if event.kind != EventKind::MessageAdded {
        return None;
    }
    if event.metadata.get("internal").map(String::as_str) == Some("true") {
        return None;
    }

    Some(ChatMessageView {
        role: event.metadata.get("role")?.to_string(),
        content: redact_sensitive_text(event.metadata.get("content")?),
        timestamp_ms: event.timestamp_ms,
    })
}

fn message_from_event(event: &Event) -> Option<Message> {
    if event.kind != EventKind::MessageAdded {
        return None;
    }
    if event.metadata.get("internal").map(String::as_str) == Some("true") {
        return None;
    }

    Some(Message {
        role: message_role_from_label(event.metadata.get("role")?)?,
        content: redact_sensitive_text(event.metadata.get("content")?),
        metadata: redact_metadata(&event.metadata),
    })
}

fn tool_run_from_event(event: &Event) -> Option<ToolRunView> {
    if event.kind != EventKind::ToolCallFinished {
        return None;
    }

    Some(ToolRunView {
        invocation_id: event.metadata.get("tool_call_id")?.to_string(),
        tool_name: event.metadata.get("tool")?.to_string(),
        status: event.metadata.get("status")?.to_string(),
        output: event
            .metadata
            .get("output")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        timestamp_ms: event.timestamp_ms,
    })
}

fn tool_approval_from_audit(record: PermissionAuditRecord) -> Option<ToolApprovalView> {
    Some(ToolApprovalView {
        request_id: record.request.id.0,
        invocation_id: record.request.metadata.get("tool_call_id")?.to_string(),
        tool_name: record.request.metadata.get("tool_name")?.to_string(),
        risk: permission_risk_label(&record.request.risk).to_string(),
        reason: redact_sensitive_text(&record.request.reason),
        scope: redact_sensitive_text(&record.request.scope),
        input: record
            .request
            .metadata
            .get("tool_input")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        requested_at_ms: record.requested_at_ms,
    })
}

fn orchestration_step_from_event(event: &Event) -> Option<OrchestrationStepView> {
    if event.kind != EventKind::ModelRequestFinished {
        return None;
    }
    let orchestration_id = event.metadata.get("orchestration_id")?.to_string();

    Some(OrchestrationStepView {
        orchestration_id,
        policy: event.metadata.get("policy")?.to_string(),
        step_index: event.metadata.get("step_index")?.parse().ok()?,
        role: event.metadata.get("role")?.to_string(),
        model: event.metadata.get("model")?.to_string(),
        output: event
            .metadata
            .get("output")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        latency_ms: event
            .metadata
            .get("latency_ms")
            .and_then(|value| value.parse().ok()),
        timestamp_ms: event.timestamp_ms,
    })
}

#[cfg(test)]
fn rag_sources_from_results(results: &[RagSearchResult]) -> Vec<RagSourceView> {
    results
        .iter()
        .map(|result| RagSourceView {
            path: result.chunk.path.clone(),
            start_line: result.chunk.start_line,
            end_line: result.chunk.end_line,
            file_hash: result.chunk.file_hash.clone(),
            score: result.score,
            reason: "vector_seed".to_string(),
            text: result.chunk.text.clone(),
        })
        .collect()
}

fn rag_sources_from_graph_trace(trace: &GraphRagTrace) -> Vec<RagSourceView> {
    trace
        .selected
        .iter()
        .map(|source| RagSourceView {
            path: source.chunk.path.clone(),
            start_line: source.chunk.start_line,
            end_line: source.chunk.end_line,
            file_hash: source.chunk.file_hash.clone(),
            score: source.score,
            reason: source.reason.clone(),
            text: source.chunk.text.clone(),
        })
        .collect()
}

fn rag_results_from_graph_trace(trace: &GraphRagTrace) -> Vec<RagSearchResult> {
    trace
        .selected
        .iter()
        .map(|source| RagSearchResult {
            chunk: source.chunk.clone(),
            score: source.score,
        })
        .collect()
}

fn rag_stats_view(stats: &RagIndexStats) -> RagStatsView {
    RagStatsView {
        files_indexed: stats.files_indexed,
        chunks_indexed: stats.chunks_indexed,
        indexed_at_ms: stats.indexed_at_ms,
    }
}

fn rag_answer_from_event(event: &Event) -> Option<String> {
    if event.kind != EventKind::ModelRequestFinished {
        return None;
    }

    event
        .metadata
        .get("answer")
        .map(|value| redact_sensitive_text(value))
}

fn append_rag_retrieval_event(
    store: &mut SqliteStore,
    action: &str,
    query: &str,
    results: &[RagSearchResult],
    trace: Option<&GraphRagTrace>,
) -> Result<(), StorageError> {
    let top_source = results.first().map(|result| {
        format!(
            "{}:{}-{}",
            result.chunk.path, result.chunk.start_line, result.chunk.end_line
        )
    });
    let mut metadata = [
        ("action".to_string(), action.to_string()),
        ("query".to_string(), query.to_string()),
        ("result_count".to_string(), results.len().to_string()),
        ("top_source".to_string(), top_source.unwrap_or_default()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(trace) = trace {
        metadata.insert("retrieval_mode".to_string(), "graph_rag".to_string());
        metadata.insert("vector_seed_count".to_string(), trace.seeds.len().to_string());
        metadata.insert(
            "graph_neighbor_count".to_string(),
            trace.neighbors.len().to_string(),
        );
        metadata.insert("selected_count".to_string(), trace.selected.len().to_string());
    }

    append_event(
        store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        format!("RAG {action} completed"),
        metadata,
    )
}

struct CloudRagEmbedder {
    config: ProviderConfig,
}

impl RagEmbedder for CloudRagEmbedder {
    fn embed_texts(&mut self, texts: &[String]) -> Result<EmbeddingBatch, agent_rag::RagError> {
        let model = self.config.model_for_role(&ModelRole::Embedder);
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: self.config.base_url.clone(),
            api_key: self.config.api_key.clone(),
            model: self.config.model_for_role(&ModelRole::Executor),
            embedding_model: model.clone(),
            timeout_seconds: 180,
        });
        let response = provider
            .embed(EmbeddingRequest {
                input: texts.to_vec(),
                dimensions: None,
                metadata: Metadata::new(),
            })
            .map_err(|error| agent_rag::RagError::new(error.to_string()))?;

        Ok(EmbeddingBatch {
            provider: response
                .metadata
                .get("provider")
                .cloned()
                .unwrap_or_else(|| "openai-compatible".to_string()),
            model: response.model,
            vectors: response
                .vectors
                .into_iter()
                .map(|vector| vector.embedding)
                .collect(),
        })
    }
}

fn browser_observation_from_event(event: &Event) -> Option<BrowserObservationView> {
    if event.kind != EventKind::ToolCallFinished {
        return None;
    }
    let tool_name = event.metadata.get("tool")?.to_string();
    if !is_phase8_tool(&tool_name) {
        return None;
    }

    Some(BrowserObservationView {
        invocation_id: event.metadata.get("tool_call_id")?.to_string(),
        tool_name,
        status: event.metadata.get("status")?.to_string(),
        url: event
            .metadata
            .get("result_url")
            .map(|value| redact_sensitive_text(value)),
        output: event
            .metadata
            .get("output")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        artifact_path: event.metadata.get("result_artifact_path").cloned(),
        text_path: event.metadata.get("result_text_path").cloned(),
        capture_kind: event.metadata.get("result_capture_kind").cloned(),
        timestamp_ms: event.timestamp_ms,
    })
}

fn is_phase8_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "web.search"
            | "browser.open"
            | "browser.extract_text"
            | "browser.capture"
            | "browser.click"
            | "browser.type"
            | "browser.scroll"
    )
}

fn provider_config_state(config: &ProviderConfig) -> ProviderConfigState {
    ProviderConfigState {
        base_url: config.base_url.clone(),
        model: config.model.clone(),
        planner_model: config.planner_model.clone(),
        executor_model: config.executor_model.clone(),
        reviewer_model: config.reviewer_model.clone(),
        summarizer_model: config.summarizer_model.clone(),
        embedding_model: config.embedding_model.clone(),
        collaboration_policy: config.collaboration_policy.clone(),
        context_window_tokens: config.context_window_tokens,
        agent_system_prompt: config.agent_system_prompt.clone(),
        api_key_set: !config.api_key.trim().is_empty(),
    }
}

fn model_candidates_for_config(config: &ProviderConfig) -> Vec<ModelCandidate> {
    [
        (ModelRole::Planner, config.model_for_role(&ModelRole::Planner), 3, 2),
        (ModelRole::Executor, config.model_for_role(&ModelRole::Executor), 2, 1),
        (ModelRole::Reviewer, config.model_for_role(&ModelRole::Reviewer), 2, 2),
        (
            ModelRole::Summarizer,
            config.model_for_role(&ModelRole::Summarizer),
            1,
            1,
        ),
    ]
    .into_iter()
    .map(|(role, name, cost_tier, latency_tier)| ModelCandidate {
        name,
        role,
        supports_tools: true,
        supports_vision: true,
        cost_tier,
        latency_tier,
    })
    .collect()
}

fn orchestration_model_for_step(
    config: &ProviderConfig,
    role: &ModelRole,
    routing_decision: &RoutingDecision,
) -> String {
    if *role == ModelRole::Executor && !routing_decision.model.trim().is_empty() {
        routing_decision.model.clone()
    } else {
        config.model_for_role(role)
    }
}

fn clone_provider_config(state: &tauri::State<'_, AppState>) -> Result<ProviderConfig, String> {
    state
        .provider_config
        .lock()
        .map(|config| config.clone())
        .map_err(|error| format!("provider config lock poisoned: {error}"))
}

fn apply_provider_config_input(config: &mut ProviderConfig, input: ProviderConfigInput) {
    config.base_url = normalized_config_value(&input.base_url);
    config.model = normalized_config_value(&input.model);
    config.planner_model = normalized_config_value(&input.planner_model);
    config.executor_model = normalized_config_value(&input.executor_model);
    config.reviewer_model = normalized_config_value(&input.reviewer_model);
    config.summarizer_model = normalized_config_value(&input.summarizer_model);
    config.embedding_model = normalized_config_value(&input.embedding_model);
    config.collaboration_policy = match input.collaboration_policy.as_str() {
        "single" | "plan_execute_review" | "best_of_n" | "auto_router" => {
            input.collaboration_policy
        }
        _ => "auto_router".to_string(),
    };
    config.context_window_tokens = input.context_window_tokens.max(4_096);
    config.agent_system_prompt = normalized_agent_system_prompt(&input.agent_system_prompt);
    let api_key = normalized_config_value(&input.api_key);
    if !api_key.is_empty() {
        config.api_key = api_key;
    }

    if config.planner_model.is_empty() {
        config.planner_model = config.model.clone();
    }
    if config.executor_model.is_empty() {
        config.executor_model = config.model.clone();
    }
    if config.reviewer_model.is_empty() {
        config.reviewer_model = config.model.clone();
    }
    if config.summarizer_model.is_empty() {
        config.summarizer_model = config.model.clone();
    }
    if config.embedding_model.is_empty() {
        config.embedding_model = "text-embedding-3-small".to_string();
    }
}

fn load_provider_config() -> ProviderConfig {
    let mut config = ProviderConfig::default();
    let Ok(text) = fs::read_to_string(provider_config_path()) else {
        return config;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "base_url" => config.base_url = value.to_string(),
            "api_key" => config.api_key = value.to_string(),
            "model" => config.model = value.to_string(),
            "planner_model" => config.planner_model = value.to_string(),
            "executor_model" => config.executor_model = value.to_string(),
            "reviewer_model" => config.reviewer_model = value.to_string(),
            "summarizer_model" => config.summarizer_model = value.to_string(),
            "embedding_model" => config.embedding_model = value.to_string(),
            "collaboration_policy" => config.collaboration_policy = value.to_string(),
            "context_window_tokens" => {
                config.context_window_tokens = value.parse().unwrap_or(128_000)
            }
            "agent_system_prompt_hex" => {
                if let Some(prompt) = config_hex_decode(value) {
                    config.agent_system_prompt = normalized_agent_system_prompt(&prompt);
                }
            }
            _ => {}
        }
    }

    config
}

fn save_provider_config_to_disk(config: &ProviderConfig) -> Result<(), std::io::Error> {
    let path = provider_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(
        format!(
            "base_url={}\napi_key={}\nmodel={}\nplanner_model={}\nexecutor_model={}\nreviewer_model={}\nsummarizer_model={}\nembedding_model={}\ncollaboration_policy={}\ncontext_window_tokens={}\nagent_system_prompt_hex={}\n",
            sanitize_config_value(&config.base_url),
            sanitize_config_value(&config.api_key),
            sanitize_config_value(&config.model),
            sanitize_config_value(&config.planner_model),
            sanitize_config_value(&config.executor_model),
            sanitize_config_value(&config.reviewer_model),
            sanitize_config_value(&config.summarizer_model),
            sanitize_config_value(&config.embedding_model),
            sanitize_config_value(&config.collaboration_policy),
            config.context_window_tokens,
            config_hex_encode(&config.agent_system_prompt)
        )
        .as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

fn load_workspace_config() -> WorkspaceConfig {
    let mut config = WorkspaceConfig::default();
    let Ok(text) = fs::read_to_string(workspace_config_path()) else {
        return config;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "root" {
            if let Ok(root) = validate_workspace_root(value) {
                config.root = root;
            }
        }
    }

    config
}

fn save_workspace_config_to_disk(config: &WorkspaceConfig) -> Result<(), std::io::Error> {
    let path = workspace_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(format!("root={}\n", config.root.display()).as_bytes())?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

fn load_project_session_config(fallback_root: &Path) -> ProjectSessionConfig {
    let mut config = ProjectSessionConfig::default_for_root(fallback_root);
    let Ok(text) = fs::read_to_string(project_session_config_path()) else {
        return config;
    };

    let mut active_project_id = String::new();
    let mut active_session_id = String::new();
    let mut projects = Vec::new();
    let mut sessions = Vec::new();

    for line in text.lines() {
        if let Some(value) = line.strip_prefix("active_project_id=") {
            active_project_id = value.to_string();
            continue;
        }
        if let Some(value) = line.strip_prefix("active_session_id=") {
            active_session_id = value.to_string();
            continue;
        }
        if let Some(value) = line.strip_prefix("project\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() >= 6 {
                projects.push(ProjectRecord {
                    id: fields[0].to_string(),
                    name: fields[1].to_string(),
                    root: fields[2].to_string(),
                    detail: fields[3].to_string(),
                    created_at_ms: fields[4].parse().unwrap_or_default(),
                    updated_at_ms: fields[5].parse().unwrap_or_default(),
                });
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("session\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() >= 7 {
                sessions.push(SessionRecord {
                    id: fields[0].to_string(),
                    project_id: fields[1].to_string(),
                    name: fields[2].to_string(),
                    detail: fields[3].to_string(),
                    created_at_ms: fields[4].parse().unwrap_or_default(),
                    updated_at_ms: fields[5].parse().unwrap_or_default(),
                    archived_at_ms: fields
                        .get(7)
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|value| *value > 0),
                });
                if fields[6] == "active" {
                    active_session_id = fields[0].to_string();
                }
            }
        }
    }

    if !projects.is_empty() {
        config.projects = projects;
    }
    if !sessions.is_empty() {
        config.sessions = sessions;
    }
    if !active_project_id.is_empty() {
        config.active_project_id = active_project_id;
    }
    if !active_session_id.is_empty() {
        config.active_session_id = active_session_id;
    }
    config.ensure_consistent(fallback_root);

    config
}

fn save_project_session_config_to_disk(
    config: &ProjectSessionConfig,
) -> Result<(), std::io::Error> {
    let path = project_session_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut text = format!(
        "active_project_id={}\nactive_session_id={}\n",
        sanitize_config_value(&config.active_project_id),
        sanitize_config_value(&config.active_session_id)
    );
    for project in &config.projects {
        text.push_str(&format!(
            "project\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sanitize_record_field(&project.id),
            sanitize_record_field(&project.name),
            sanitize_record_field(&project.root),
            sanitize_record_field(&project.detail),
            project.created_at_ms,
            project.updated_at_ms
        ));
    }
    for session in &config.sessions {
        let active_marker = if session.id == config.active_session_id {
            "active"
        } else {
            ""
        };
        text.push_str(&format!(
            "session\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sanitize_record_field(&session.id),
            sanitize_record_field(&session.project_id),
            sanitize_record_field(&session.name),
            sanitize_record_field(&session.detail),
            session.created_at_ms,
            session.updated_at_ms,
            active_marker,
            session.archived_at_ms.unwrap_or_default()
        ));
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(text.as_bytes())?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

fn project_session_state(
    config: &ProjectSessionConfig,
    last_error: Option<String>,
) -> ProjectSessionState {
    ProjectSessionState {
        projects: config
            .projects
            .iter()
            .map(|project| ProjectView {
                id: project.id.clone(),
                name: project.name.clone(),
                root: project.root.clone(),
                detail: project.detail.clone(),
                status: if project.id == config.active_project_id {
                    "Selected".to_string()
                } else {
                    "Ready".to_string()
                },
                active: project.id == config.active_project_id,
                created_at_ms: project.created_at_ms,
                updated_at_ms: project.updated_at_ms,
            })
            .collect(),
        sessions: config
            .sessions
            .iter()
            .map(|session| SessionView {
                id: session.id.clone(),
                project_id: session.project_id.clone(),
                name: session.name.clone(),
                detail: session.detail.clone(),
                status: if session.id == config.active_session_id {
                    "Active".to_string()
                } else if session.archived_at_ms.is_some() {
                    "Archived".to_string()
                } else {
                    "Ready".to_string()
                },
                active: session.id == config.active_session_id,
                archived: session.archived_at_ms.is_some(),
                archived_at_ms: session.archived_at_ms,
                created_at_ms: session.created_at_ms,
                updated_at_ms: session.updated_at_ms,
            })
            .collect(),
        active_project_id: config.active_project_id.clone(),
        active_session_id: config.active_session_id.clone(),
        last_error,
    }
}

fn ensure_open_session_for_project(
    config: &mut ProjectSessionConfig,
    project_id: &str,
) -> String {
    if let Some(session) = config
        .sessions
        .iter()
        .find(|session| session.project_id == project_id && session.archived_at_ms.is_none())
    {
        return session.id.clone();
    }

    let project_name = config
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .map(|project| project.name.clone())
        .unwrap_or_else(|| "Runtime".to_string());
    let name = format!("{project_name} Session");
    let id = unique_config_id(
        "session",
        &name,
        &config
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>(),
    );
    let now = current_time_millis();
    config.sessions.push(SessionRecord {
        id: id.clone(),
        project_id: project_id.to_string(),
        name,
        detail: "timeline + chat".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        archived_at_ms: None,
    });
    id
}

fn unique_fork_name(config: &ProjectSessionConfig, source: &SessionRecord) -> String {
    let base = format!("{} Fork", source.name);
    if !config
        .sessions
        .iter()
        .any(|session| session.project_id == source.project_id && session.name == base)
    {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{} Fork {suffix}", source.name);
        if !config
            .sessions
            .iter()
            .any(|session| session.project_id == source.project_id && session.name == candidate)
        {
            return candidate;
        }
        suffix += 1;
    }
}

fn project_session_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<ProjectSessionState, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    Ok(project_session_state(&config, Some(message.into())))
}

fn sync_active_project_root(
    state: &tauri::State<'_, AppState>,
    root: &Path,
) -> Result<(), String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let active_project_id = config.active_project_id.clone();
    if let Some(project) = config
        .projects
        .iter_mut()
        .find(|project| project.id == active_project_id)
    {
        project.root = root.display().to_string();
        project.updated_at_ms = current_time_millis();
        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn project_session_metadata_for_session(
    state: &tauri::State<'_, AppState>,
    session_id: Option<&str>,
) -> Result<Metadata, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    let Some(session_id) = session_id else {
        return Ok(project_session_metadata(&config));
    };
    let session = config
        .sessions
        .iter()
        .find(|session| session.id == session_id && session.archived_at_ms.is_none())
        .ok_or_else(|| format!("session not found: {session_id}"))?;
    let project = config
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .ok_or_else(|| format!("project not found for session: {session_id}"))?;

    Ok([
        ("project_id".to_string(), project.id.clone()),
        ("project_name".to_string(), project.name.clone()),
        ("project_root".to_string(), project.root.clone()),
        ("session_id".to_string(), session.id.clone()),
        ("session_name".to_string(), session.name.clone()),
    ]
    .into_iter()
    .collect())
}

fn project_session_metadata(config: &ProjectSessionConfig) -> Metadata {
    let mut metadata = Metadata::new();
    if let Some(project) = config.active_project() {
        metadata.insert("project_id".to_string(), project.id.clone());
        metadata.insert("project_name".to_string(), project.name.clone());
        metadata.insert("project_root".to_string(), project.root.clone());
    }
    if let Some(session) = config.active_session() {
        metadata.insert("session_id".to_string(), session.id.clone());
        metadata.insert("session_name".to_string(), session.name.clone());
    }
    metadata
}

fn metadata_with_context(mut metadata: Metadata, context: &Metadata) -> Metadata {
    for (key, value) in context {
        metadata.entry(key.clone()).or_insert_with(|| value.clone());
    }
    metadata
}

fn agent_run_context_from_events(events: &[Event]) -> AgentRunContext {
    let start = events.iter().find(|event| is_agent_run_start_event(event));
    AgentRunContext {
        project_id: start.and_then(|event| event.metadata.get("project_id").cloned()),
        project_name: start.and_then(|event| event.metadata.get("project_name").cloned()),
        session_id: start.and_then(|event| event.metadata.get("session_id").cloned()),
        session_name: start.and_then(|event| event.metadata.get("session_name").cloned()),
    }
}

#[derive(Debug, Clone, Default)]
struct AgentRunContext {
    project_id: Option<String>,
    project_name: Option<String>,
    session_id: Option<String>,
    session_name: Option<String>,
}

fn load_sidecar_config() -> SidecarConfig {
    let mut config = SidecarConfig::default();
    let Ok(text) = fs::read_to_string(sidecar_config_path()) else {
        return config;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "browser_path" => config.browser_path = value.to_string(),
            "computer_path" => config.computer_path = value.to_string(),
            "auto_configure" => config.auto_configure = config_bool(value),
            _ => {}
        }
    }

    if config.browser_path.trim().is_empty() {
        config.browser_path = default_browser_sidecar_path().display().to_string();
    }
    if config.computer_path.trim().is_empty() {
        config.computer_path = default_computer_sidecar_path().display().to_string();
    }

    config
}

fn save_sidecar_config_to_disk(config: &SidecarConfig) -> Result<(), std::io::Error> {
    let path = sidecar_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(
        format!(
            "browser_path={}\ncomputer_path={}\nauto_configure={}\n",
            sanitize_config_value(&config.browser_path),
            sanitize_config_value(&config.computer_path),
            config.auto_configure
        )
        .as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

fn apply_sidecar_env(config: &SidecarConfig) {
    if config.auto_configure {
        std::env::set_var("CINDX_BROWSER_SIDECAR", &config.browser_path);
        std::env::set_var("CINDX_COMPUTER_SIDECAR", &config.computer_path);
        if let Some(node_path) = find_node_executable() {
            std::env::set_var("CINDX_NODE", node_path);
        }
    } else {
        std::env::remove_var("CINDX_BROWSER_SIDECAR");
        std::env::remove_var("CINDX_COMPUTER_SIDECAR");
        std::env::remove_var("CINDX_NODE");
    }
}

fn sidecar_state(config: &SidecarConfig, last_error: Option<String>) -> SidecarState {
    SidecarState {
        browser: sidecar_endpoint_state("CINDX_BROWSER_SIDECAR", &config.browser_path),
        computer: sidecar_endpoint_state("CINDX_COMPUTER_SIDECAR", &config.computer_path),
        auto_configure: config.auto_configure,
        last_error,
    }
}

fn sidecar_endpoint_state(env_key: &str, path: &str) -> SidecarEndpointState {
    let path_buf = PathBuf::from(path);
    let exists = path_buf.exists();
    let executable = exists && path_buf.is_file();
    let health = if executable {
        let mut command = sidecar_command(&path_buf);
        command
            .arg("--health")
            .output()
            .map(|output| {
                (
                    output.status.success(),
                    if output.status.success() {
                        String::from_utf8_lossy(&output.stdout).trim().to_string()
                    } else {
                        String::from_utf8_lossy(&output.stderr).trim().to_string()
                    },
                )
            })
            .unwrap_or_else(|error| (false, error.to_string()))
    } else {
        (false, "sidecar path is missing".to_string())
    };

    SidecarEndpointState {
        path: path.to_string(),
        exists,
        executable,
        healthy: health.0,
        health_output: health.1,
        env_key: env_key.to_string(),
    }
}

fn sidecar_command(path: &Path) -> std::process::Command {
    if path.extension().and_then(|extension| extension.to_str()) == Some("js") {
        let mut command = std::process::Command::new(
            find_node_executable().unwrap_or_else(|| PathBuf::from("node")),
        );
        command.arg(path);
        command
    } else {
        std::process::Command::new(path)
    }
}

fn find_node_executable() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CINDX_NODE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    [
        PathBuf::from("/opt/homebrew/bin/node"),
        PathBuf::from("/usr/local/bin/node"),
        PathBuf::from("/usr/bin/node"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn default_browser_sidecar_path() -> PathBuf {
    bundled_or_development_resource("browser-sidecar.js")
}

fn default_computer_sidecar_path() -> PathBuf {
    bundled_or_development_resource("computer-sidecar.js")
}

fn development_repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

fn bundled_or_development_resource(file_name: &str) -> PathBuf {
    let packaged = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .and_then(|macos| macos.parent().map(Path::to_path_buf))
        .map(|contents| contents.join("Resources").join("sidecars").join(file_name));
    if let Some(path) = packaged.as_ref().filter(|path| path.is_file()) {
        return path.clone();
    }

    let development = development_repo_root()
        .join("scripts")
        .join("sidecars")
        .join(file_name);
    if development.is_file() {
        return development;
    }

    packaged.unwrap_or(development)
}

fn config_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y"
    )
}

fn validate_workspace_root(path: &str) -> Result<PathBuf, String> {
    let path = normalized_config_value(path);
    if path.is_empty() {
        return Err("workspace path is empty".to_string());
    }
    let candidate = PathBuf::from(path);
    let root = if candidate.is_absolute() {
        candidate
    } else {
        workspace_root().join(candidate)
    };
    let canonical =
        fs::canonicalize(&root).map_err(|error| format!("failed to resolve workspace: {error}"))?;
    if !canonical.is_dir() {
        return Err("workspace path must be a directory".to_string());
    }

    Ok(canonical)
}

fn active_workspace_root(state: &tauri::State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .workspace_config
        .lock()
        .map(|config| config.root.clone())
        .map_err(|error| format!("workspace config lock poisoned: {error}"))
}

fn tool_registry_for_state(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<ToolRegistry, String> {
    let mut registry = ToolRegistry::with_workspace_tools(workspace_root.to_path_buf());
    let catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    for tool in catalog.cached_tools() {
        registry.register(tool);
    }
    drop(catalog);
    for tool in skill_catalog_for_root(workspace_root).tools() {
        registry.register(tool);
    }
    registry.install_meta_tools();
    Ok(registry)
}

fn skill_catalog_for_root(workspace_root: &Path) -> SkillCatalog {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.to_path_buf());
    SkillCatalog::load(
        home.join(".cindx/skills"),
        workspace_root,
        home.join(".cindx/skill-preferences.json"),
    )
}

fn open_app_store() -> Result<SqliteStore, StorageError> {
    let database_path = database_path();
    if let Some(parent) = database_path.parent() {
        fs::create_dir_all(parent).map_err(|error| StorageError::new(error.to_string()))?;
        secure_directory(parent).map_err(|error| StorageError::new(error.to_string()))?;
    }

    let store = SqliteStore::open(&database_path)?;
    #[cfg(unix)]
    fs::set_permissions(&database_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| StorageError::new(error.to_string()))?;

    Ok(store)
}

fn database_path() -> PathBuf {
    app_data_root().join("state.sqlite3")
}

fn provider_config_path() -> PathBuf {
    app_data_root().join("provider.conf")
}

fn workspace_config_path() -> PathBuf {
    app_data_root().join("workspace.conf")
}

fn project_session_config_path() -> PathBuf {
    app_data_root().join("projects.conf")
}

fn sidecar_config_path() -> PathBuf {
    app_data_root().join("sidecars.conf")
}

fn mcp_config_path() -> PathBuf {
    app_data_root().join("mcp-servers.json")
}

fn mcp_catalog_cache_path() -> PathBuf {
    app_data_root().join("mcp-catalog.json")
}

fn rag_index_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("rag-index.tsv")
}

fn lancedb_export_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".cindx")
        .join("lancedb-records.jsonl")
}

fn graph_store_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("graph.tsv")
}

fn context_checkpoint_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".cindx")
        .join("context-checkpoint.md")
}

fn agent_trace_export_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("agent-trace.jsonl")
}

fn open_rag_adapter_for(workspace_root: &Path) -> Result<FileRagAdapter, String> {
    FileRagAdapter::open(rag_index_path_for(workspace_root)).map_err(|error| error.to_string())
}

fn index_graph_chunks(workspace_root: &Path, chunks: &[RagChunk]) -> Result<(usize, usize), String> {
    let graph_path = graph_store_path_for(workspace_root);
    if graph_path.exists() {
        fs::remove_file(&graph_path)
            .map_err(|error| format!("failed to reset graph store: {error}"))?;
    }
    let mut graph_store = FileGraphStore::open(&graph_path).map_err(|error| error.to_string())?;
    for chunk in chunks {
        graph_store
            .upsert(extract_graph_from_chunk(chunk))
            .map_err(|error| error.to_string())?;
    }

    Ok((graph_store.nodes().len(), graph_store.edges().len()))
}

fn graph_rag_trace_for(
    workspace_root: &Path,
    adapter: &FileRagAdapter,
    query: &str,
    seed_results: &[RagSearchResult],
    limit: usize,
) -> Result<GraphRagTrace, String> {
    let graph_store =
        FileGraphStore::open(graph_store_path_for(workspace_root)).map_err(|error| error.to_string())?;
    Ok(graph_rag_walk(
        query,
        seed_results,
        adapter.chunks(),
        &graph_store,
        limit,
    ))
}

fn workspace_root() -> PathBuf {
    if let Some(root) = runtime_path_from_env("CINDX_DEFAULT_WORKSPACE") {
        if root.is_dir() {
            return root;
        }
    }
    if let Some(home) = user_home_directory() {
        if home.is_dir() {
            return home;
        }
    }
    std::env::current_dir()
        .ok()
        .filter(|path| path.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

fn app_data_root() -> PathBuf {
    app_data_root_for(
        runtime_path_from_env("CINDX_DATA_DIR"),
        user_home_directory(),
    )
}

fn app_data_root_for(override_root: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    if let Some(root) = override_root {
        return root;
    }
    if let Some(home) = home {
        #[cfg(target_os = "macos")]
        return home.join("Library").join("Application Support").join("Cindx");
        #[cfg(not(target_os = "macos"))]
        return home.join(".cindx");
    }
    std::env::temp_dir().join("Cindx")
}

fn runtime_path_from_env(key: &str) -> Option<PathBuf> {
    let value = std::env::var_os(key)?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Some(path)
    } else {
        std::env::current_dir().ok().map(|current| current.join(path))
    }
}

fn user_home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn migrate_legacy_app_data() -> Result<(), std::io::Error> {
    if runtime_path_from_env("CINDX_DATA_DIR").is_some() {
        return Ok(());
    }
    let source = development_repo_root().join(".cindx");
    let destination = app_data_root();
    if source == destination || !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(&destination)?;
    secure_directory(&destination)?;
    for file_name in [
        "state.sqlite3",
        "state.sqlite3-shm",
        "state.sqlite3-wal",
        "provider.conf",
        "workspace.conf",
        "projects.conf",
        "sidecars.conf",
        "mcp-servers.json",
        "mcp-catalog.json",
    ] {
        let source_path = source.join(file_name);
        let destination_path = destination.join(file_name);
        if source_path.is_file() && !destination_path.exists() {
            fs::copy(&source_path, &destination_path)?;
            secure_private_file(&destination_path)?;
        }
    }
    Ok(())
}

fn secure_directory(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn secure_private_file(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn append_startup_log(message: &str) {
    let root = app_data_root();
    if fs::create_dir_all(&root).is_err() {
        return;
    }
    let _ = secure_directory(&root);
    let path = root.join("startup.log");
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let Ok(mut file) = options.open(&path) else {
        return;
    };
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let _ = writeln!(file, "{timestamp_ms} {message}");
    let _ = secure_private_file(&path);
}

fn install_startup_panic_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        append_startup_log(&format!("panic: {info}"));
        previous(info);
    }));
}

fn phase3_task_id() -> TaskId {
    TaskId(PHASE3_TASK_ID.to_string())
}

fn phase4_task_id() -> TaskId {
    TaskId(PHASE4_TASK_ID.to_string())
}

fn phase5_task_id() -> TaskId {
    TaskId(PHASE5_TASK_ID.to_string())
}

fn phase6_task_id() -> TaskId {
    TaskId(PHASE6_TASK_ID.to_string())
}

fn phase7_task_id() -> TaskId {
    TaskId(PHASE7_TASK_ID.to_string())
}

fn phase8_task_id() -> TaskId {
    TaskId(PHASE8_TASK_ID.to_string())
}

fn phase15_task_id() -> TaskId {
    TaskId(PHASE15_TASK_ID.to_string())
}

fn phase16_task_id() -> TaskId {
    TaskId(PHASE16_TASK_ID.to_string())
}

fn context_task_ids() -> Vec<TaskId> {
    vec![
        phase3_task_id(),
        phase4_task_id(),
        phase5_task_id(),
        phase6_task_id(),
        phase7_task_id(),
        phase8_task_id(),
        phase15_task_id(),
        phase16_task_id(),
    ]
}

fn unique_id(prefix: &str) -> String {
    let counter = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", current_time_millis())
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_millis() as u64
}

fn parse_permission_decision(value: &str) -> Result<PermissionDecision, StorageError> {
    match value {
        "allow_once" => Ok(PermissionDecision::AllowOnce),
        "allow_for_session" => Ok(PermissionDecision::AllowForSession),
        "deny" => Ok(PermissionDecision::Deny),
        other => Err(StorageError::new(format!("unknown permission decision: {other}"))),
    }
}

fn event_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::TaskStatusChanged => "Status",
        EventKind::ToolCallProposed => "Tool proposed",
        EventKind::PermissionRequested => "Permission requested",
        EventKind::PermissionResolved => "Permission resolved",
        EventKind::ToolCallStarted => "Tool started",
        EventKind::ToolCallFinished => "Tool finished",
        EventKind::ModelRequestStarted => "Model started",
        EventKind::ModelRequestFinished => "Model finished",
        EventKind::RetrievalPerformed => "Retrieval",
        EventKind::MessageAdded => "Message",
        EventKind::Error => "Error",
        _ => "Runtime event",
    }
}

fn event_kind_ui_kind(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::ToolCallProposed | EventKind::ToolCallStarted | EventKind::ToolCallFinished => {
            "tool"
        }
        EventKind::PermissionRequested | EventKind::PermissionResolved => "permission",
        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished => "model",
        EventKind::RetrievalPerformed => "tool",
        _ => "message",
    }
}

fn event_state(kind: &EventKind, permission_is_pending: bool) -> &'static str {
    match kind {
        EventKind::PermissionRequested if permission_is_pending => "pending",
        EventKind::ToolCallProposed if permission_is_pending => "pending",
        EventKind::Error => "pending",
        _ => "done",
    }
}

fn message_role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::Reviewer => "reviewer",
    }
}

fn message_role_from_label(value: &str) -> Option<MessageRole> {
    match value {
        "system" => Some(MessageRole::System),
        "user" => Some(MessageRole::User),
        "assistant" => Some(MessageRole::Assistant),
        "tool" => Some(MessageRole::Tool),
        "reviewer" => Some(MessageRole::Reviewer),
        _ => None,
    }
}

fn tool_risk_label(risk: &ToolRisk) -> &'static str {
    match risk {
        ToolRisk::ReadOnly => "read_only",
        ToolRisk::WritesWorkspace => "writes_workspace",
        ToolRisk::ExecutesProcess => "executes_process",
        ToolRisk::UsesNetwork => "uses_network",
        ToolRisk::SensitiveContext => "sensitive_context",
        ToolRisk::Destructive => "destructive",
    }
}

fn tool_outcome_label(status: &ToolOutcomeStatus) -> &'static str {
    match status {
        ToolOutcomeStatus::Succeeded => "succeeded",
        ToolOutcomeStatus::Failed => "failed",
        ToolOutcomeStatus::Cancelled => "cancelled",
        ToolOutcomeStatus::Denied => "denied",
    }
}

fn permission_risk_label(risk: &PermissionRisk) -> &'static str {
    match risk {
        PermissionRisk::Read => "read",
        PermissionRisk::Write => "write",
        PermissionRisk::Execute => "execute",
        PermissionRisk::Network => "network",
        PermissionRisk::Sensitive => "sensitive",
        PermissionRisk::Destructive => "destructive",
    }
}

fn permission_decision_label(decision: &PermissionDecision) -> &'static str {
    match decision {
        PermissionDecision::AllowOnce => "allow_once",
        PermissionDecision::AllowForSession => "allow_for_session",
        PermissionDecision::Deny => "deny",
    }
}

fn permission_decision_past_tense(decision: &PermissionDecision) -> &'static str {
    match decision {
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession => "approved",
        PermissionDecision::Deny => "denied",
    }
}

fn normalized_config_value(value: &str) -> String {
    sanitize_config_value(value.trim())
}

fn normalized_agent_system_prompt(value: &str) -> String {
    let prompt = value.trim();
    if prompt.is_empty() {
        DEFAULT_AGENT_SYSTEM_PROMPT.to_string()
    } else {
        prompt.chars().take(32_000).collect()
    }
}

fn config_hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn config_hex_decode(value: &str) -> Option<String> {
    if value.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

fn sanitize_config_value(value: &str) -> String {
    value.replace(['\n', '\r'], "")
}

fn sanitize_record_field(value: &str) -> String {
    sanitize_config_value(value).replace('\t', " ")
}

fn unique_config_id(prefix: &str, label: &str, existing: &[String]) -> String {
    let slug = slug_label(label);
    let base = format!("{prefix}-{slug}");
    if !existing.iter().any(|id| id == &base) {
        return base;
    }
    for index in 2..1000 {
        let candidate = format!("{base}-{index}");
        if !existing.iter().any(|id| id == &candidate) {
            return candidate;
        }
    }

    format!("{base}-{}", current_time_millis())
}

fn slug_label(label: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "item".to_string()
    } else {
        slug
    }
}

fn truncate_for_timeline(value: &str) -> String {
    const LIMIT: usize = 160;
    if value.chars().count() <= LIMIT {
        return value.to_string();
    }

    let mut truncated = value.chars().take(LIMIT).collect::<String>();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::encode_input;

    #[test]
    fn installed_app_data_is_user_scoped_and_overrideable() {
        let home = PathBuf::from("/Users/new-cindx-user");
        let expected = home
            .join("Library")
            .join("Application Support")
            .join("Cindx");
        assert_eq!(app_data_root_for(None, Some(home)), expected);

        let override_root = PathBuf::from("/tmp/cindx-portable-data");
        assert_eq!(
            app_data_root_for(Some(override_root.clone()), None),
            override_root
        );
        assert_eq!(database_path(), app_data_root().join("state.sqlite3"));
    }

    #[test]
    fn runtime_status_exposes_expected_modes() {
        let status = runtime_status_for_root(workspace_root());

        assert_eq!(status.app_version, env!("CARGO_PKG_VERSION"));
        assert!(status
            .orchestration_modes
            .contains(&"plan_execute_review".to_string()));
        assert!(status.registered_tools.contains(&"shell.run".to_string()));
    }

    #[test]
    fn quit_confirmation_preference_requires_explicit_suppression() {
        assert!(quit_confirmation_suppressed_text(
            "theme=system\nskip_quit_confirmation=true\n"
        ));
        assert!(!quit_confirmation_suppressed_text(
            "skip_quit_confirmation=false\n"
        ));
    }

    #[test]
    fn phase3_mock_permission_round_trips_through_store() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let state = request_mock_permission_in_store(&mut store).expect("request should save");

        assert_eq!(state.permissions.len(), 1);
        assert_eq!(state.permissions[0].status, "pending");

        let request_id = state.permissions[0].id.clone();
        let resolved = resolve_permission_in_store(&mut store, &request_id, "deny")
            .expect("resolution should save");

        assert_eq!(resolved.permissions.len(), 1);
        assert_eq!(resolved.permissions[0].status, "resolved");
        assert_eq!(resolved.permissions[0].decision.as_deref(), Some("deny"));
        assert!(resolved
            .timeline
            .iter()
            .any(|entry| entry.label == "Permission resolved"));
    }

    #[test]
    fn phase4_state_includes_provider_config_and_messages() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let config = ProviderConfig {
            api_key: "secret".to_string(),
            ..ProviderConfig::default()
        };
        append_message_event(
            &mut store,
            &phase4_task_id(),
            MessageRole::User,
            "hello model",
        )
        .expect("message should append");

        let state = phase4_state(&store, &config, None).expect("state should load");

        assert!(state.provider.api_key_set);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].content, "hello model");
    }

    #[test]
    fn redacts_sensitive_values_in_new_and_existing_events() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_message_event(
            &mut store,
            &phase4_task_id(),
            MessageRole::Tool,
            "api_key=secret-value\nBearer token-value\nsk-1234567890abcdef",
        )
        .expect("message should append");
        store
            .append(Event {
                id: EventId("legacy-secret".to_string()),
                task_id: phase4_task_id(),
                sequence: 2,
                timestamp_ms: 20,
                kind: EventKind::ToolCallFinished,
                summary: "legacy secret".to_string(),
                metadata: [("output".to_string(), "password=old-secret".to_string())]
                    .into_iter()
                    .collect(),
            })
            .expect("legacy event should append");

        let updated = redact_persisted_events(&mut store).expect("history should redact");
        let events = store
            .list_by_task(&phase4_task_id())
            .expect("events should load");
        let rendered = events
            .iter()
            .flat_map(|event| event.metadata.values())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(updated, 1);
        assert!(!rendered.contains("secret-value"));
        assert!(!rendered.contains("token-value"));
        assert!(!rendered.contains("1234567890abcdef"));
        assert!(!rendered.contains("old-secret"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn provider_config_input_preserves_existing_key_when_blank() {
        let mut config = ProviderConfig {
            api_key: "existing".to_string(),
            ..ProviderConfig::default()
        };

        apply_provider_config_input(
            &mut config,
            ProviderConfigInput {
                base_url: "https://example.test/v1".to_string(),
                api_key: "".to_string(),
                model: "model-a".to_string(),
                planner_model: "".to_string(),
                executor_model: "".to_string(),
                reviewer_model: "".to_string(),
                summarizer_model: "".to_string(),
                embedding_model: "".to_string(),
                collaboration_policy: "auto_router".to_string(),
                context_window_tokens: 128_000,
                agent_system_prompt: "Be concise.\nUse Chinese when asked.".to_string(),
            },
        );

        assert_eq!(config.api_key, "existing");
        assert_eq!(config.executor_model, "model-a");
        assert_eq!(config.collaboration_policy, "auto_router");
        assert_eq!(config.context_window_tokens, 128_000);
        assert_eq!(config.agent_system_prompt, "Be concise.\nUse Chinese when asked.");
    }

    #[test]
    fn agent_system_prompt_config_encoding_preserves_multiline_unicode() {
        let prompt = "你是 Cindx。\n先检查事实，再执行。";
        let encoded = config_hex_encode(prompt);

        assert_eq!(config_hex_decode(&encoded).as_deref(), Some(prompt));
        assert!(config_hex_decode("not-hex").is_none());
    }

    #[test]
    fn agent_state_reports_context_usage_and_hides_internal_drafts() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [
                ("prompt".to_string(), "inspect".to_string()),
                ("context_window_tokens".to_string(), "100000".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("start should append");
        append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
            .expect("user message should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            [("prompt_tokens".to_string(), "25000".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("usage should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "internal draft",
            [("internal".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("draft should append");
        append_message_event(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "final answer",
        )
        .expect("final should append");

        let state = agent_state(&store, None).expect("state should load");

        assert_eq!(state.context_tokens_used, 25_000);
        assert_eq!(state.context_window_tokens, 100_000);
        assert_eq!(state.context_remaining_percent, 75.0);
        assert!(!state.context_usage_estimated);
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[1].content, "final answer");
    }

    #[test]
    fn phase5_state_lists_tool_results() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let root = workspace_root();
        execute_tool_invocation(
            &mut store,
            ToolInvocation {
                id: agent_core::ToolCallId("tool-1".to_string()),
                task_id: phase5_task_id(),
                tool_name: "file.list".to_string(),
                input_json: encode_input(&[("path", ".")]),
                proposed_by_model: "test".to_string(),
                metadata: Metadata::new(),
            },
            &root,
        )
        .expect("tool should execute");

        let state = phase5_state(&store, None, &root).expect("state should load");

        assert!(state.tools.iter().any(|tool| tool.name == "file.write"));
        assert_eq!(state.results.len(), 1);
        assert_eq!(state.results[0].tool_name, "file.list");
    }

    #[test]
    fn phase5_state_lists_pending_tool_approvals() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let root = workspace_root();
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId("tool-2".to_string()),
            task_id: phase5_task_id(),
            tool_name: "file.write".to_string(),
            input_json: encode_input(&[("path", ".cindx/phase5-test.txt"), ("content", "ok")]),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        };
        let registry = ToolRegistry::with_workspace_tools(root.clone());
        let mut request = registry
            .get("file.write")
            .expect("tool should exist")
            .permission_request(&invocation)
            .expect("write should request permission");
        request.id = PermissionRequestId("perm-phase5".to_string());
        request.metadata.insert("phase".to_string(), "5".to_string());
        request
            .metadata
            .insert("tool_input".to_string(), invocation.input_json);
        request
            .metadata
            .insert("tool_call_id".to_string(), invocation.id.0);
        request
            .metadata
            .insert("tool_name".to_string(), invocation.tool_name);
        store
            .save_permission_request(request, 123)
            .expect("request should save");

        let state = phase5_state(&store, None, &root).expect("state should load");

        assert_eq!(state.pending_approvals.len(), 1);
        assert_eq!(state.pending_approvals[0].tool_name, "file.write");
    }

    #[test]
    fn phase6_state_lists_orchestration_steps() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase6_task_id(),
            EventKind::ModelRequestFinished,
            "planner step finished",
            [
                ("orchestration_id".to_string(), "orch-1".to_string()),
                ("policy".to_string(), "plan_execute_review".to_string()),
                ("step_index".to_string(), "0".to_string()),
                ("role".to_string(), "planner".to_string()),
                ("model".to_string(), "model-a".to_string()),
                ("latency_ms".to_string(), "12".to_string()),
                ("output".to_string(), "Plan first.".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("event should append");

        let state = phase6_state(&store, None).expect("state should load");

        assert_eq!(state.steps.len(), 1);
        assert_eq!(state.steps[0].policy, "plan_execute_review");
        assert_eq!(state.steps[0].role, "planner");
        assert_eq!(state.steps[0].output, "Plan first.");
    }

    #[test]
    fn phase7_state_reports_rag_stats() {
        let root = temp_test_root("phase7-stats");
        fs::create_dir_all(&root).expect("temp root should exist");
        fs::write(
            root.join("notes.md"),
            "# Cindx\n\nThe RAG index stores line-level provenance.",
        )
        .expect("fixture should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let mut adapter =
            FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
                .expect("adapter should open");
        adapter.replace_all(index).expect("index should persist");
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase7_task_id(),
            EventKind::RetrievalPerformed,
            "Workspace indexed for RAG",
            [
                ("action".to_string(), "index".to_string()),
                ("files_indexed".to_string(), "1".to_string()),
                ("chunks_indexed".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("event should append");

        let state =
            phase7_state(&store, &adapter, Vec::new(), None, None).expect("state should load");

        assert_eq!(state.stats.files_indexed, 1);
        assert_eq!(state.stats.chunks_indexed, 1);
        assert!(state.timeline.iter().any(|entry| entry.label == "Retrieval"));
    }

    #[test]
    fn rag_sources_include_line_ranges() {
        let root = temp_test_root("phase7-sources");
        fs::create_dir_all(root.join("docs")).expect("temp docs should exist");
        fs::write(
            root.join("docs").join("rag.md"),
            "Intro\nSemantic retrieval should cite exact source lines.\nDone",
        )
        .expect("fixture should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let mut adapter =
            FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
                .expect("adapter should open");
        adapter.replace_all(index).expect("index should persist");

        let results = adapter
            .search("semantic retrieval source lines", 3)
            .expect("search should run");
        let sources = rag_sources_from_results(&results);

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].path, "docs/rag.md");
        assert_eq!(sources[0].start_line, 1);
        assert_eq!(sources[0].end_line, 3);
        assert!(sources[0].score > 0.0);
    }

    #[test]
    fn context_state_builds_and_persists_restore_pack() {
        let root = temp_test_root("phase15-context");
        fs::create_dir_all(&root).expect("temp root should exist");
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_message_event(
            &mut store,
            &phase4_task_id(),
            MessageRole::User,
            "Continue the MVP context manager",
        )
        .expect("message should append");
        append_event(
            &mut store,
            &phase7_task_id(),
            EventKind::RetrievalPerformed,
            "RAG search completed",
            [
                ("action".to_string(), "search".to_string()),
                ("query".to_string(), "context compression".to_string()),
                ("selected_count".to_string(), "2".to_string()),
                ("retrieval_mode".to_string(), "graph_rag".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("retrieval should append");

        let preview = context_state(&store, &root, None, None).expect("state should load");
        let checkpoint = preview.checkpoint.expect("checkpoint should exist");

        assert_eq!(
            checkpoint.current_goal.as_deref(),
            Some("Continue the MVP context manager")
        );
        assert!(checkpoint.path.is_none());
        assert!(checkpoint.restore_pack.contains("## Current Goal"));
        assert!(checkpoint.restore_pack.contains("graph_rag"));

        let events = collect_context_events(&store).expect("events should collect");
        let pack = build_restore_context_pack(build_session_checkpoint_at(
            &events,
            CheckpointOptions::default(),
            777,
        ));
        let checkpoint_path =
            write_context_checkpoint(&root, &pack.text).expect("checkpoint should write");
        append_event(
            &mut store,
            &phase15_task_id(),
            EventKind::TaskStatusChanged,
            "Context checkpoint compacted",
            [
                ("checkpoint_id".to_string(), pack.checkpoint.id.clone()),
                (
                    "context_checkpoint_path".to_string(),
                    checkpoint_path.display().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        )
        .expect("compact event should append");

        let compacted = context_state(&store, &root, Some(pack), None).expect("state should load");
        let checkpoint = compacted.checkpoint.expect("checkpoint should exist");
        let expected_path = checkpoint_path.display().to_string();

        assert_eq!(checkpoint.path.as_deref(), Some(expected_path.as_str()));
        assert!(fs::read_to_string(checkpoint_path)
            .expect("checkpoint should be readable")
            .contains("Cindx Context Checkpoint"));
        assert!(compacted
            .timeline
            .iter()
            .any(|entry| entry.detail.contains("Context checkpoint compacted")));
    }

    #[test]
    fn phase8_state_lists_browser_observations() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase8_task_id(),
            EventKind::ToolCallFinished,
            "Browser text extracted",
            [
                ("tool_call_id".to_string(), "browser-1".to_string()),
                ("tool".to_string(), "browser.extract_text".to_string()),
                ("status".to_string(), "succeeded".to_string()),
                ("output".to_string(), "Example Domain".to_string()),
                ("result_url".to_string(), "https://example.com".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("event should append");

        let state = phase8_state(&store, None).expect("state should load");

        assert_eq!(state.observations.len(), 1);
        assert_eq!(state.observations[0].tool_name, "browser.extract_text");
        assert_eq!(state.observations[0].url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn phase8_state_lists_pending_browser_approvals() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId("browser-2".to_string()),
            task_id: phase8_task_id(),
            tool_name: "browser.capture".to_string(),
            input_json: encode_input(&[("url", "https://example.com")]),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        };
        let registry = ToolRegistry::with_workspace_tools(workspace_root());
        let mut request = registry
            .get("browser.capture")
            .expect("tool should exist")
            .permission_request(&invocation)
            .expect("browser capture should request permission");
        request.id = PermissionRequestId("perm-phase8".to_string());
        request.metadata.insert("phase".to_string(), "8".to_string());
        request
            .metadata
            .insert("tool_input".to_string(), invocation.input_json);
        request
            .metadata
            .insert("tool_call_id".to_string(), invocation.id.0);
        request
            .metadata
            .insert("tool_name".to_string(), invocation.tool_name);
        store
            .save_permission_request(request, 456)
            .expect("request should save");

        let state = phase8_state(&store, None).expect("state should load");

        assert_eq!(state.pending_approvals.len(), 1);
        assert_eq!(state.pending_approvals[0].tool_name, "browser.capture");
    }

    #[test]
    fn agent_state_lists_pending_agent_approvals() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("prompt".to_string(), "write a file".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("start should append");
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId("agent-tool-1".to_string()),
            task_id: phase16_task_id(),
            tool_name: "file.write".to_string(),
            input_json: encode_input(&[("path", ".cindx/agent-loop.txt"), ("content", "ok")]),
            proposed_by_model: "agent-loop".to_string(),
            metadata: Metadata::new(),
        };
        let registry = ToolRegistry::with_workspace_tools(workspace_root());
        let mut request = registry
            .get("file.write")
            .expect("tool should exist")
            .permission_request(&invocation)
            .expect("write should request permission");
        request.id = PermissionRequestId("perm-agent".to_string());
        request.metadata.insert("phase".to_string(), "16".to_string());
        request
            .metadata
            .insert("tool_input".to_string(), invocation.input_json);
        request
            .metadata
            .insert("tool_call_id".to_string(), invocation.id.0);
        request
            .metadata
            .insert("tool_name".to_string(), invocation.tool_name);
        request
            .metadata
            .insert("agent_prompt".to_string(), "write a file".to_string());
        store
            .save_permission_request(request, current_time_millis())
            .expect("request should save");

        let state = agent_state(&store, None).expect("state should load");

        assert_eq!(state.status, "waiting_for_permission");
        assert_eq!(state.pending_approvals.len(), 1);
        assert_eq!(state.pending_approvals[0].tool_name, "file.write");

        store
            .resolve_permission(PermissionResolution {
                request_id: PermissionRequestId("perm-agent".to_string()),
                decision: PermissionDecision::AllowOnce,
                resolved_at_ms: current_time_millis(),
                resolved_by: "local-user".to_string(),
            })
            .expect("permission should resolve");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task resumed after permission",
            Metadata::new(),
        )
        .expect("resume should append");

        let resumed = agent_state(&store, None).expect("resumed state should load");
        assert_eq!(resumed.status, "running");
        assert!(resumed.pending_approvals.is_empty());
    }

    #[test]
    fn agent_transcript_restores_assistant_tool_and_tool_messages() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("prompt".to_string(), "read README".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("start should append");
        append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "read README")
            .expect("user message should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "",
            [
                (
                    "raw_tool_calls_json".to_string(),
                    r#"[{"id":"call-1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]"#.to_string(),
                ),
                ("tool_call_count".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("assistant tool call should append");
        append_tool_message_event(
            &mut store,
            &phase16_task_id(),
            "call-1",
            "file.read",
            "succeeded",
            "tool=file.read\nstatus=succeeded\noutput=hello",
            None,
        )
        .expect("tool message should append");

        let events = store
            .list_by_task(&phase16_task_id())
            .expect("events should load");
        let transcript = agent_transcript_from_active_events(&active_agent_events(&events));

        assert_eq!(transcript.len(), 3);
        assert!(matches!(transcript[1].role, MessageRole::Assistant));
        assert!(transcript[1].metadata.contains_key("raw_tool_calls_json"));
        assert!(matches!(transcript[2].role, MessageRole::Tool));
        assert_eq!(
            transcript[2].metadata.get("tool_call_id").map(String::as_str),
            Some("call-1")
        );

        let state = agent_state(&store, None).expect("agent state should load");
        assert_eq!(state.messages.len(), 3);
        assert_eq!(state.messages[0].role, "user");
        assert_eq!(state.messages[2].role, "tool");
    }

    #[test]
    fn agent_state_and_trace_are_isolated_by_session() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        for (session_id, prompt, answer) in [
            ("session-a", "inspect alpha", "alpha answer"),
            ("session-b", "inspect beta", "beta answer"),
            ("session-a", "follow up alpha", "second alpha answer"),
        ] {
            let context = [
                ("project_id".to_string(), "project-cindx".to_string()),
                ("project_name".to_string(), "Cindx".to_string()),
                ("session_id".to_string(), session_id.to_string()),
                ("session_name".to_string(), session_id.to_string()),
            ]
            .into_iter()
            .collect::<Metadata>();
            let mut start_metadata = context.clone();
            start_metadata.insert("prompt".to_string(), prompt.to_string());
            append_event(
                &mut store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                "Agent task started",
                start_metadata,
            )
            .expect("start should append");
            append_message_event_with_metadata(
                &mut store,
                &phase16_task_id(),
                MessageRole::User,
                prompt,
                context.clone(),
            )
            .expect("user message should append");
            append_message_event_with_metadata(
                &mut store,
                &phase16_task_id(),
                MessageRole::Assistant,
                answer,
                context,
            )
            .expect("assistant message should append");
        }

        let alpha = agent_state_for_session(&store, None, Some("session-a"))
            .expect("alpha state should load");
        let beta = agent_state_for_session(&store, None, Some("session-b"))
            .expect("beta state should load");
        let alpha_trace = agent_trace_state_for_session(&store, None, None, Some("session-a"))
            .expect("alpha trace should load");

        assert_eq!(alpha.session_id.as_deref(), Some("session-a"));
        assert_eq!(alpha.messages.len(), 4);
        assert_eq!(alpha.messages[1].content, "alpha answer");
        assert_eq!(alpha.messages[3].content, "second alpha answer");
        assert_eq!(beta.session_id.as_deref(), Some("session-b"));
        assert_eq!(beta.messages.len(), 2);
        assert_eq!(beta.messages[1].content, "beta answer");
        assert_eq!(alpha_trace.session_id.as_deref(), Some("session-a"));
        assert!(alpha_trace
            .turns
            .iter()
            .flat_map(|turn| turn.steps.iter())
            .all(|step| !step.detail.contains("beta")));
    }

    #[test]
    fn interleaved_agent_runs_remain_isolated_by_session() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let alpha = [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect::<Metadata>();
        let beta = [("session_id".to_string(), "session-b".to_string())]
            .into_iter()
            .collect::<Metadata>();

        for (summary, context) in [
            ("Agent task started", alpha.clone()),
            ("Agent task started", beta.clone()),
        ] {
            append_event(
                &mut store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                summary,
                context,
            )
            .expect("run start should append");
        }
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "alpha finished after beta started",
            alpha.clone(),
        )
        .expect("alpha answer should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "beta answer",
            beta.clone(),
        )
        .expect("beta answer should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task completed",
            alpha,
        )
        .expect("alpha completion should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task completed",
            beta,
        )
        .expect("beta completion should append");

        let alpha_state = agent_state_for_session(&store, None, Some("session-a"))
            .expect("alpha state should load");
        let beta_state = agent_state_for_session(&store, None, Some("session-b"))
            .expect("beta state should load");

        assert_eq!(alpha_state.status, "completed");
        assert_eq!(alpha_state.messages.len(), 1);
        assert_eq!(alpha_state.messages[0].content, "alpha finished after beta started");
        assert_eq!(beta_state.status, "completed");
        assert_eq!(beta_state.messages.len(), 1);
        assert_eq!(beta_state.messages[0].content, "beta answer");
    }

    #[test]
    fn cancelling_one_session_does_not_cancel_another() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        for session_id in ["session-a", "session-b"] {
            append_event(
                &mut store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                "Agent task started",
                [("session_id".to_string(), session_id.to_string())]
                    .into_iter()
                    .collect(),
            )
            .expect("run start should append");
        }
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task cancelled",
            [("session_id".to_string(), "session-a".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("cancellation should append");

        assert!(agent_task_is_cancelled(&store, Some("session-a"))
            .expect("alpha cancellation should load"));
        assert!(!agent_task_is_cancelled(&store, Some("session-b"))
            .expect("beta cancellation should load"));
    }

    #[test]
    fn session_permission_grant_is_reused_only_for_matching_scope() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let granted = PermissionRequest {
            id: PermissionRequestId("session-grant".to_string()),
            task_id: phase16_task_id(),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "run a command".to_string(),
            scope: ".".to_string(),
            metadata: [
                ("session_id".to_string(), "session-a".to_string()),
                ("agent_run_id".to_string(), "run-a".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        store
            .save_permission_request(granted.clone(), 1)
            .expect("grant request should save");
        store
            .resolve_permission(PermissionResolution {
                request_id: granted.id.clone(),
                decision: PermissionDecision::AllowForSession,
                resolved_at_ms: 2,
                resolved_by: "local-user".to_string(),
            })
            .expect("grant should resolve");

        let mut next = granted.clone();
        next.id = PermissionRequestId("next-request".to_string());
        assert!(agent_session_permission_granted(&store, &next, Some("session-a"))
            .expect("matching grant should load"));
        assert!(!agent_session_permission_granted(&store, &next, Some("session-b"))
            .expect("other session should load"));
        next.scope = "crates/tools".to_string();
        assert!(!agent_session_permission_granted(&store, &next, Some("session-a"))
            .expect("other scope should load"));
        next.scope = ".".to_string();
        next.risk = PermissionRisk::Destructive;
        assert!(!agent_session_permission_granted(&store, &next, Some("session-a"))
            .expect("destructive grant should not persist"));
    }

    #[test]
    fn pending_permissions_are_isolated_by_agent_run() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        for (id, run_id) in [("pending-old", "run-old"), ("pending-new", "run-new")] {
            store
                .save_permission_request(
                    PermissionRequest {
                        id: PermissionRequestId(id.to_string()),
                        task_id: phase16_task_id(),
                        risk: PermissionRisk::Execute,
                        action: "shell.run".to_string(),
                        reason: "run a command".to_string(),
                        scope: ".".to_string(),
                        metadata: [
                            ("session_id".to_string(), "session-a".to_string()),
                            ("agent_run_id".to_string(), run_id.to_string()),
                        ]
                        .into_iter()
                        .collect(),
                    },
                    if run_id == "run-old" { 1 } else { 2 },
                )
                .expect("pending request should save");
        }

        let pending = pending_agent_permissions_for_run(
            &store,
            Some("session-a"),
            Some("run-new"),
        )
        .expect("pending requests should load");

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id.0, "pending-new");
    }

    #[test]
    fn agent_state_ignores_old_pending_approvals_after_new_run() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let mut request = PermissionRequest {
            id: PermissionRequestId("old-agent-perm".to_string()),
            task_id: phase16_task_id(),
            risk: PermissionRisk::Write,
            action: "file.write".to_string(),
            reason: "old run".to_string(),
            scope: ".".to_string(),
            metadata: [
                ("tool_call_id".to_string(), "old-call".to_string()),
                ("tool_name".to_string(), "file.write".to_string()),
                ("tool_input".to_string(), "path=old.txt".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        request.metadata.insert("phase".to_string(), "16".to_string());
        store
            .save_permission_request(request, 1)
            .expect("old request should save");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("prompt".to_string(), "new task".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("start should append");
        append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "new task")
            .expect("user message should append");

        let state = agent_state(&store, None).expect("state should load");

        assert_eq!(state.status, "running");
        assert!(state.pending_approvals.is_empty());
        assert_eq!(state.transcript_messages, 1);
    }

    #[test]
    fn agent_state_reports_cancelled_and_retryable() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("prompt".to_string(), "inspect".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("start should append");
        append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
            .expect("user message should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task cancelled",
            Metadata::new(),
        )
        .expect("cancel should append");

        let state = agent_state(&store, None).expect("state should load");

        assert_eq!(state.status, "cancelled");
        assert!(state.can_retry);
        assert!(!state.can_cancel);
    }

    #[test]
    fn agent_trace_groups_steps_by_turn_and_exposes_details() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("prompt".to_string(), "read README".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("start should append");
        append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "read README")
            .expect("user message should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestStarted,
            "Agent model turn started",
            [
                ("request_id".to_string(), "agent-model-1".to_string()),
                ("turn".to_string(), "0".to_string()),
                ("model".to_string(), "model-a".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("model start should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            [
                ("request_id".to_string(), "agent-model-1".to_string()),
                ("latency_ms".to_string(), "42".to_string()),
                ("tool_calls".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("model finish should append");
        append_tool_finished_event(
            &mut store,
            &phase16_task_id(),
            "call-1",
            "file.write",
            "succeeded",
            "written ok",
            [("path".to_string(), "notes/result.md".to_string())]
                .into_iter()
                .collect(),
            None,
        )
        .expect("tool finish should append");

        let trace = agent_trace_state_for_session(&store, None, None, None)
            .expect("trace should build");

        assert_eq!(trace.turn_count, 1);
        assert!(trace.step_count >= 5);
        assert!(trace.tool_call_count >= 1);
        assert!(trace
            .turns
            .iter()
            .any(|turn| turn.index == 1 && turn.steps.iter().any(|step| {
                step.kind == "model" && step.latency_ms == Some(42)
            })));
        assert!(trace
            .turns
            .iter()
            .flat_map(|turn| turn.steps.iter())
            .any(|step| step.tool_name.as_deref() == Some("file.write")
                && step.output_preview.as_deref() == Some("written ok")
                && step.artifact_path.as_deref() == Some("notes/result.md")));
    }

    #[test]
    fn agent_trace_export_writes_jsonl() {
        let root = temp_test_root("phase18-trace");
        fs::create_dir_all(&root).expect("temp root should exist");
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("prompt".to_string(), "inspect".to_string())]
                .into_iter()
                .collect(),
        )
        .expect("start should append");
        append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
            .expect("user message should append");

        let path = write_agent_trace_jsonl(&root, &store, None).expect("trace should export");
        let text = fs::read_to_string(path).expect("trace export should be readable");

        assert!(text.contains("\"trace_id\""));
        assert!(text.contains("\"task_id\":\"phase-16-agent-loop\""));
        assert!(text.contains("\"kind\":\"message\""));
    }

    #[test]
    fn project_session_state_defaults_to_active_workspace() {
        let root = temp_test_root("phase20-projects");
        let config = ProjectSessionConfig::default_for_root(&root);
        let state = project_session_state(&config, None);

        assert_eq!(state.projects.len(), 1);
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.projects[0].root, root.display().to_string());
        assert!(state.projects[0].active);
        assert!(state.sessions[0].active);
        assert_eq!(state.active_project_id, "project-cindx");
        assert_eq!(state.active_session_id, "session-runtime");
    }

    #[test]
    fn automatic_session_names_use_the_first_prompt() {
        assert!(is_automatic_session_name("Runtime Session"));
        assert!(is_automatic_session_name("New Session"));
        assert!(!is_automatic_session_name("Release planning"));
        assert_eq!(
            automatic_session_title("  Review   the project architecture and risks  "),
            "Review the project architecture and"
        );
    }

    #[test]
    fn archived_active_session_gets_a_visible_replacement_and_can_be_listed() {
        let root = temp_test_root("archived-session");
        let mut config = ProjectSessionConfig::default_for_root(&root);
        config.sessions[0].archived_at_ms = Some(42);
        config.ensure_consistent(&root);

        let state = project_session_state(&config, None);
        let archived = state
            .sessions
            .iter()
            .find(|session| session.archived)
            .expect("archived session should remain recoverable");
        let active = state
            .sessions
            .iter()
            .find(|session| session.active)
            .expect("replacement session should be active");

        assert_eq!(archived.archived_at_ms, Some(42));
        assert_ne!(archived.id, active.id);
        assert!(!active.archived);
    }

    #[test]
    fn fork_names_are_unique_within_a_project() {
        let root = temp_test_root("fork-name");
        let mut config = ProjectSessionConfig::default_for_root(&root);
        let source = config.sessions[0].clone();
        assert_eq!(unique_fork_name(&config, &source), "Runtime Session Fork");
        config.sessions.push(SessionRecord {
            id: "fork-one".to_string(),
            project_id: source.project_id.clone(),
            name: "Runtime Session Fork".to_string(),
            detail: "fork".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            archived_at_ms: None,
        });

        assert_eq!(unique_fork_name(&config, &source), "Runtime Session Fork 2");
    }

    #[test]
    fn agent_state_and_trace_expose_project_session_context() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [
                ("prompt".to_string(), "inspect".to_string()),
                ("project_id".to_string(), "project-alpha".to_string()),
                ("project_name".to_string(), "Alpha".to_string()),
                ("session_id".to_string(), "session-alpha".to_string()),
                ("session_name".to_string(), "Alpha Session".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("start should append");
        append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
            .expect("user message should append");

        let state = agent_state(&store, None).expect("state should load");
        let trace = agent_trace_state_for_session(&store, None, None, None)
            .expect("trace should build");

        assert_eq!(state.project_id.as_deref(), Some("project-alpha"));
        assert_eq!(state.session_name.as_deref(), Some("Alpha Session"));
        assert_eq!(trace.project_name.as_deref(), Some("Alpha"));
        assert_eq!(trace.session_id.as_deref(), Some("session-alpha"));
    }

    #[test]
    fn default_sidecar_state_is_healthy() {
        let config = SidecarConfig::default();
        let state = sidecar_state(&config, None);

        assert!(state.auto_configure);
        assert!(state.browser.exists);
        assert!(state.browser.healthy);
        assert!(state.computer.exists);
        assert!(state.computer.healthy);
    }

    fn temp_test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{name}-{}", unique_id("test")))
    }
}
