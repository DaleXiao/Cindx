use super::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeStatus {
    pub(crate) app_version: String,
    pub(crate) kernel_status: String,
    pub(crate) provider_ready: bool,
    pub(crate) workspace_root: String,
    pub(crate) orchestration_modes: Vec<String>,
    pub(crate) registered_tools: Vec<String>,
    pub(crate) agent_run_budgets: AgentRunBudgetsView,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRunBudgetView {
    pub(crate) max_duration_ms: u64,
    pub(crate) max_model_calls: usize,
    pub(crate) max_tool_calls: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRunBudgetsView {
    pub(crate) fast: AgentRunBudgetView,
    pub(crate) auto: AgentRunBudgetView,
    pub(crate) pro: AgentRunBudgetView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarState {
    pub(crate) browser: SidecarEndpointState,
    pub(crate) computer: SidecarEndpointState,
    pub(crate) auto_configure: bool,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarEndpointState {
    pub(crate) path: String,
    pub(crate) exists: bool,
    pub(crate) executable: bool,
    pub(crate) healthy: bool,
    pub(crate) health_output: String,
    pub(crate) env_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectSessionState {
    pub(crate) projects: Vec<ProjectView>,
    pub(crate) sessions: Vec<SessionView>,
    pub(crate) active_project_id: String,
    pub(crate) active_session_id: String,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleStateView {
    pub(crate) schedules: Vec<ScheduleView>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) project_id: Option<String>,
    pub(crate) project_name: String,
    pub(crate) session_id: Option<String>,
    pub(crate) session_name: String,
    pub(crate) prompt: String,
    pub(crate) effort: String,
    pub(crate) timezone: String,
    pub(crate) cadence: String,
    pub(crate) anchor_at_ms: u64,
    pub(crate) weekly_days: Vec<u8>,
    pub(crate) ends_at_ms: Option<u64>,
    pub(crate) catch_up: bool,
    pub(crate) enabled: bool,
    pub(crate) next_run_at_ms: Option<u64>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) runs: Vec<ScheduleRunRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) root: String,
    pub(crate) detail: String,
    pub(crate) status: String,
    pub(crate) active: bool,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionView {
    pub(crate) id: String,
    pub(crate) project_id: String,
    pub(crate) name: String,
    pub(crate) title_state: String,
    pub(crate) detail: String,
    pub(crate) effort: String,
    pub(crate) status: String,
    pub(crate) activity: String,
    pub(crate) attention_reason: Option<String>,
    pub(crate) unseen_result: bool,
    pub(crate) latest_sequence: u64,
    pub(crate) active: bool,
    pub(crate) archived: bool,
    pub(crate) archived_at_ms: Option<u64>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceInput {
    pub(crate) path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateProjectInput {
    pub(crate) name: String,
    pub(crate) root: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateSessionInput {
    pub(crate) name: String,
    pub(crate) project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpsertScheduleInput {
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) project_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) prompt: String,
    pub(crate) effort: String,
    pub(crate) timezone: String,
    pub(crate) cadence: String,
    pub(crate) anchor_local: String,
    #[serde(default)]
    pub(crate) weekly_days: Vec<u8>,
    pub(crate) ends_local: Option<String>,
    pub(crate) catch_up: bool,
    pub(crate) enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleActionInput {
    pub(crate) schedule_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetScheduleEnabledInput {
    pub(crate) schedule_id: String,
    pub(crate) enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RenameSessionInput {
    pub(crate) session_id: String,
    pub(crate) name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionEffortInput {
    pub(crate) session_id: String,
    pub(crate) effort: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateSessionTitleInput {
    pub(crate) session_id: String,
    pub(crate) prompt: String,
    pub(crate) answer: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RenameProjectInput {
    pub(crate) project_id: String,
    pub(crate) name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectActionInput {
    pub(crate) project_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentAttachmentView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) mime_type: String,
    pub(crate) size_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawAttachmentUploadMetadata {
    pub(crate) session_id: String,
    pub(crate) batch_id: String,
    pub(crate) name: String,
    pub(crate) mime_type: String,
    pub(crate) batch_file_count: usize,
    pub(crate) batch_file_sizes: Vec<u64>,
    pub(crate) batch_index: usize,
    pub(crate) batch_total_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AbortAgentAttachmentBatchInput {
    pub(crate) session_id: String,
    pub(crate) batch_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoveAgentAttachmentInput {
    pub(crate) session_id: String,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactPreviewView {
    pub(crate) kind: String,
    pub(crate) mime_type: String,
    pub(crate) content: Option<String>,
    pub(crate) data_url: Option<String>,
    pub(crate) size_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectProjectInput {
    pub(crate) project_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectSessionInput {
    pub(crate) session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionActionInput {
    pub(crate) session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AcknowledgeSessionActivityInput {
    pub(crate) session_id: String,
    pub(crate) through_sequence: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfirmDeleteInput {
    pub(crate) kind: String,
    pub(crate) name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueAgentMessageInput {
    pub(crate) session_id: String,
    pub(crate) prompt: String,
    pub(crate) current_time: String,
    #[serde(default)]
    pub(crate) queue_id: Option<String>,
    #[serde(default = "default_agent_effort")]
    pub(crate) effort: String,
    #[serde(default)]
    pub(crate) attachments: Vec<AgentAttachmentView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueuedAgentMessageActionInput {
    pub(crate) session_id: String,
    pub(crate) queue_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditQueuedAgentMessageInput {
    pub(crate) session_id: String,
    pub(crate) queue_id: String,
    pub(crate) prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarConfigInput {
    pub(crate) browser_path: String,
    pub(crate) computer_path: String,
    pub(crate) auto_configure: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebSearchConfigState {
    pub(crate) endpoint: String,
    pub(crate) api_key_set: bool,
    pub(crate) configured: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebSearchConfigInput {
    pub(crate) endpoint: String,
    pub(crate) api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineEntry {
    pub(crate) sequence: u64,
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_name: Option<String>,
    pub(crate) state: String,
    pub(crate) timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workflow_progress: Option<WorkflowProgressView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowProgressView {
    pub(crate) completed_steps: usize,
    pub(crate) total_steps: usize,
    pub(crate) current_step_id: Option<String>,
    pub(crate) step_status: Option<String>,
    pub(crate) continuations: usize,
    pub(crate) recoverable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermissionAudit {
    pub(crate) id: String,
    pub(crate) risk: String,
    pub(crate) action: String,
    pub(crate) reason: String,
    pub(crate) scope: String,
    pub(crate) status: String,
    pub(crate) decision: Option<String>,
    pub(crate) requested_at_ms: u64,
    pub(crate) resolved_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Phase3State {
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) permissions: Vec<PermissionAudit>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermissionReviewItem {
    pub(crate) request_id: String,
    pub(crate) action: String,
    pub(crate) risk: String,
    pub(crate) reason: String,
    pub(crate) scope: String,
    pub(crate) source: String,
    pub(crate) project_id: Option<String>,
    pub(crate) project_name: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) session_name: Option<String>,
    pub(crate) input: String,
    pub(crate) requested_at_ms: u64,
    pub(crate) can_allow_session: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermissionReviewState {
    pub(crate) pending: Vec<PermissionReviewItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderConfigState {
    pub(crate) provider_id: String,
    pub(crate) provider_resource: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) conductor_model: String,
    pub(crate) planner_model: String,
    pub(crate) executor_model: String,
    pub(crate) reviewer_model: String,
    pub(crate) summarizer_model: String,
    pub(crate) fast_model: String,
    pub(crate) auto_model: String,
    pub(crate) pro_model: String,
    pub(crate) embedding_model: String,
    pub(crate) image_model: String,
    pub(crate) image_endpoint: String,
    pub(crate) voice_model: String,
    pub(crate) collaboration_policy: String,
    pub(crate) prompt_evolution_enabled: bool,
    pub(crate) context_window_tokens: u64,
    pub(crate) agent_system_prompt: String,
    pub(crate) ready: bool,
    pub(crate) api_key_set: bool,
    pub(crate) auth_verified: bool,
    pub(crate) auth_verified_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatMessageView {
    pub(crate) sequence: u64,
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) timestamp_ms: u64,
    pub(crate) run_id: Option<String>,
    pub(crate) queue_id: Option<String>,
    pub(crate) attachments: Vec<AgentAttachmentView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Phase4State {
    pub(crate) provider: ProviderConfigState,
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) messages: Vec<ChatMessageView>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolSpecView {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) risk: String,
    pub(crate) input_schema: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolRunView {
    pub(crate) invocation_id: String,
    pub(crate) tool_name: String,
    pub(crate) status: String,
    pub(crate) output: String,
    pub(crate) timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolApprovalView {
    pub(crate) request_id: String,
    pub(crate) invocation_id: String,
    pub(crate) tool_name: String,
    pub(crate) risk: String,
    pub(crate) reason: String,
    pub(crate) scope: String,
    pub(crate) input: String,
    pub(crate) requested_at_ms: u64,
    pub(crate) can_allow_session: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Phase5State {
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) tools: Vec<ToolSpecView>,
    pub(crate) pending_approvals: Vec<ToolApprovalView>,
    pub(crate) results: Vec<ToolRunView>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolRunInput {
    pub(crate) tool_name: String,
    pub(crate) input: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RagStatsView {
    pub(crate) files_indexed: usize,
    pub(crate) chunks_indexed: usize,
    pub(crate) indexed_at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryStatsView {
    pub(crate) records: usize,
    pub(crate) requirements: usize,
    pub(crate) outcomes: usize,
    pub(crate) evidence: usize,
    pub(crate) recalls: u64,
    pub(crate) observed_uses: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RagSourceView {
    pub(crate) path: String,
    pub(crate) start_line: u64,
    pub(crate) end_line: u64,
    pub(crate) file_hash: String,
    pub(crate) score: f32,
    pub(crate) reason: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RetrievalChannelView {
    pub(crate) name: String,
    pub(crate) result_count: usize,
    pub(crate) duration_ms: u64,
    pub(crate) top_sources: Vec<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RetrievalTraceView {
    pub(crate) query: String,
    pub(crate) mode: String,
    pub(crate) channels: Vec<RetrievalChannelView>,
    pub(crate) selected_count: usize,
    pub(crate) duration_ms: u64,
    pub(crate) index_cache_hit: bool,
    pub(crate) index_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphNodeView {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) source_path: String,
    pub(crate) focused: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphEdgeView {
    pub(crate) id: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphStateView {
    pub(crate) total_nodes: usize,
    pub(crate) total_edges: usize,
    pub(crate) nodes: Vec<GraphNodeView>,
    pub(crate) edges: Vec<GraphEdgeView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Phase7State {
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) stats: RagStatsView,
    pub(crate) memory: MemoryStatsView,
    pub(crate) sources: Vec<RagSourceView>,
    pub(crate) retrieval_trace: Option<RetrievalTraceView>,
    pub(crate) graph: GraphStateView,
    pub(crate) answer: Option<String>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RagOperationProgress {
    pub(crate) operation_id: String,
    pub(crate) operation_kind: String,
    pub(crate) stage: String,
    pub(crate) completed_steps: usize,
    pub(crate) total_steps: usize,
    pub(crate) detail: String,
    pub(crate) status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RagOperationInput {
    pub(crate) operation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RagSearchInput {
    pub(crate) operation_id: String,
    pub(crate) query: String,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserObservationView {
    pub(crate) invocation_id: String,
    pub(crate) tool_name: String,
    pub(crate) status: String,
    pub(crate) url: Option<String>,
    pub(crate) output: String,
    pub(crate) artifact_path: Option<String>,
    pub(crate) text_path: Option<String>,
    pub(crate) capture_kind: Option<String>,
    pub(crate) timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Phase8State {
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) pending_approvals: Vec<ToolApprovalView>,
    pub(crate) observations: Vec<BrowserObservationView>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextCheckpointView {
    pub(crate) id: String,
    pub(crate) generated_at_ms: u64,
    pub(crate) event_count: usize,
    pub(crate) task_count: usize,
    pub(crate) latest_event_ms: u64,
    pub(crate) current_goal: Option<String>,
    pub(crate) completed_steps: Vec<String>,
    pub(crate) pending_steps: Vec<String>,
    pub(crate) decisions: Vec<String>,
    pub(crate) file_changes: Vec<String>,
    pub(crate) commands_run: Vec<String>,
    pub(crate) tool_results: Vec<String>,
    pub(crate) retrievals: Vec<String>,
    pub(crate) artifacts: Vec<String>,
    pub(crate) errors: Vec<String>,
    pub(crate) next_actions: Vec<String>,
    pub(crate) path: Option<String>,
    pub(crate) restore_pack: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextState {
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) checkpoint: Option<ContextCheckpointView>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentState {
    pub(crate) task_id: String,
    pub(crate) project_id: Option<String>,
    pub(crate) project_name: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) session_name: Option<String>,
    pub(crate) status: String,
    pub(crate) turn_count: usize,
    pub(crate) max_turns: usize,
    pub(crate) transcript_messages: usize,
    pub(crate) context_tokens_used: u64,
    pub(crate) context_window_tokens: u64,
    pub(crate) context_remaining_percent: f64,
    pub(crate) context_usage_estimated: bool,
    pub(crate) run_started_at_ms: u64,
    pub(crate) run_budget_ms: u64,
    pub(crate) run_model_call_budget: usize,
    pub(crate) run_tool_call_budget: usize,
    pub(crate) can_cancel: bool,
    pub(crate) can_retry: bool,
    pub(crate) can_continue: bool,
    pub(crate) event_count: u64,
    pub(crate) latest_sequence: u64,
    pub(crate) oldest_sequence: u64,
    pub(crate) has_older_history: bool,
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) messages: Vec<ChatMessageView>,
    pub(crate) pending_approvals: Vec<ToolApprovalView>,
    #[serde(default)]
    pub(crate) queued_messages: Vec<QueuedAgentMessageView>,
    pub(crate) latest_answer: Option<String>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentStateRevision {
    pub(crate) session_id: String,
    pub(crate) event_count: u64,
    pub(crate) latest_sequence: u64,
    pub(crate) latest_timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentStateDelta {
    pub(crate) reset: bool,
    pub(crate) latest_sequence: u64,
    pub(crate) state: AgentState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentHistoryPage {
    pub(crate) session_id: String,
    pub(crate) oldest_sequence: u64,
    pub(crate) has_older_history: bool,
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) messages: Vec<ChatMessageView>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RoutingTelemetryEntry {
    pub(crate) run_id: String,
    pub(crate) telemetry: RoutingTelemetry,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RoutingTelemetryReadModel {
    pub(crate) schema: String,
    pub(crate) revision: u64,
    pub(crate) event_count: u64,
    pub(crate) entries: Vec<RoutingTelemetryEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTraceState {
    pub(crate) task_id: String,
    pub(crate) trace_id: String,
    pub(crate) run_id: String,
    pub(crate) project_id: Option<String>,
    pub(crate) project_name: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) session_name: Option<String>,
    pub(crate) status: String,
    pub(crate) started_at_ms: u64,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) turn_count: usize,
    pub(crate) step_count: usize,
    pub(crate) tool_call_count: usize,
    pub(crate) permission_wait_count: usize,
    pub(crate) error_count: usize,
    pub(crate) role_summaries: Vec<AgentTraceRoleSummaryView>,
    pub(crate) export_path: Option<String>,
    pub(crate) turns: Vec<AgentTraceTurnView>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTraceRoleSummaryView {
    pub(crate) role: String,
    pub(crate) models: Vec<String>,
    pub(crate) calls: usize,
    pub(crate) completed: usize,
    pub(crate) interrupted: usize,
    pub(crate) degraded: usize,
    pub(crate) latency_ms: u64,
    pub(crate) first_token_latency_ms: Option<u64>,
    pub(crate) total_tokens: u64,
    pub(crate) evidence_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTraceTurnView {
    pub(crate) index: usize,
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) started_at_ms: u64,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) steps: Vec<AgentTraceStepView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTraceStepView {
    pub(crate) id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) turn_index: usize,
    pub(crate) sequence: u64,
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) started_at_ms: u64,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) latency_ms: Option<u64>,
    pub(crate) model: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) request_id: Option<String>,
    pub(crate) tool_call_id: Option<String>,
    pub(crate) permission_id: Option<String>,
    pub(crate) input_preview: Option<String>,
    pub(crate) output_preview: Option<String>,
    pub(crate) artifact_path: Option<String>,
    pub(crate) detail: String,
    pub(crate) metadata: Metadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTaskInput {
    pub(crate) prompt: String,
    pub(crate) session_id: String,
    pub(crate) current_time: String,
    #[serde(default)]
    pub(crate) queue_id: Option<String>,
    #[serde(default = "default_agent_effort")]
    pub(crate) effort: String,
    #[serde(default)]
    pub(crate) attachments: Vec<AgentAttachmentView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserToolInput {
    pub(crate) tool_name: String,
    pub(crate) input: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderConfigInput {
    #[serde(default)]
    pub(crate) provider_id: String,
    #[serde(default)]
    pub(crate) provider_resource: String,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) conductor_model: String,
    pub(crate) planner_model: String,
    pub(crate) executor_model: String,
    pub(crate) reviewer_model: String,
    pub(crate) summarizer_model: String,
    #[serde(default)]
    pub(crate) fast_model: String,
    #[serde(default)]
    pub(crate) auto_model: String,
    #[serde(default)]
    pub(crate) pro_model: String,
    pub(crate) embedding_model: String,
    #[serde(default)]
    pub(crate) image_model: String,
    #[serde(default)]
    pub(crate) image_endpoint: String,
    #[serde(default)]
    pub(crate) voice_model: String,
    pub(crate) collaboration_policy: String,
    #[serde(default = "default_prompt_evolution_enabled")]
    pub(crate) prompt_evolution_enabled: bool,
    #[serde(default)]
    pub(crate) workflow_enabled: bool,
    pub(crate) context_window_tokens: u64,
    pub(crate) agent_system_prompt: String,
}

pub(crate) fn default_prompt_evolution_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderModelsInput {
    #[serde(default)]
    pub(crate) provider_id: String,
    #[serde(default)]
    pub(crate) provider_resource: String,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderModelsState {
    pub(crate) models: Vec<String>,
    pub(crate) fetched_at_ms: u64,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageEndpointValidationInput {
    #[serde(default)]
    pub(crate) provider_id: String,
    #[serde(default)]
    pub(crate) provider_resource: String,
    pub(crate) base_url: String,
    pub(crate) image_model: String,
    pub(crate) image_endpoint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageEndpointValidationState {
    pub(crate) endpoint: String,
    pub(crate) valid: bool,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelStreamDelta {
    pub(crate) task_id: String,
    pub(crate) request_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) delta: String,
    pub(crate) done: bool,
    pub(crate) reset: bool,
    pub(crate) error: Option<String>,
}
