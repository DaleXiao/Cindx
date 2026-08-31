#[cfg(test)]
pub(crate) use agent_application::project_agent_artifacts as agent_output_artifacts_from_events;
pub(crate) use agent_application::{
    artifact_manifest_message, AgentOutputArtifact as AgentOutputArtifactView,
    AgentRecoveryIdentity, AgentRecoveryReason, AgentRecoveryState, SessionTitleState,
};
#[cfg(test)]
pub(crate) use agent_core::EventId;
pub(crate) use agent_core::{
    AgentActor, AgentEffectAuthority, AgentModelAttribution, AgentModelProfile, AgentStage, Event,
    EventKind, Message, MessageRole, Metadata, ModelRole, PermissionDecision, PermissionRequest,
    PermissionRequestId, PermissionResolution, PermissionRisk, TaskId, ToolArtifact, ToolContent,
    ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};
pub(crate) use agent_graph::{
    extract_graph_from_chunk, graph_direct_recall, graph_walk_recall, FileGraphStore,
};
pub(crate) use agent_mcp::{McpCatalogService, McpServerConfig};
pub(crate) use agent_memory::record_memory_observed_uses;
pub(crate) use agent_memory::{
    build_restore_context_pack, build_session_checkpoint_at, conversation_memory_to_markdown,
    fuse_memory_recalls_at, memory_recalls_to_markdown, recall_memories_at, record_memory_recalls,
    CheckpointOptions, MemoryKind, MemoryLedger, RestoreContextPack, SessionCheckpoint,
};
#[cfg(test)]
pub(crate) use agent_rag::RagAdapter;
pub(crate) use agent_rag::{
    apply_embeddings_to_placeholder_chunks_cancellable, build_grounded_answer_prompt,
    export_lancedb_records_jsonl_cancellable,
    fuse_retrieval_channels_for_query as fuse_rag_retrieval_channels_for_query,
    index_workspace_reusing_cancellable, lancedb_index_exists, local_query_embedding,
    read_file_rag_stats, remove_file_rag_generation_if_unleased, replace_lancedb_index_cancellable,
    restore_local_embeddings, retrieval_ranges_overlap, search_chunks_literal,
    search_lancedb_index, workspace_index_is_fresh, EmbeddingBatch, FileRagAdapter, IndexOptions,
    RagChunk, RagEmbedder, RagError, RagIndex, RagIndexStats, RagSearchResult,
    RetrievalChannelOutcome, RAG_INDEX_CANCELLED,
};
#[cfg(test)]
pub(crate) use agent_rag::{index_workspace, replace_lancedb_index};
#[cfg(test)]
pub(crate) use agent_runtime::{
    advance_with_model_response, model_request_for_turn_with_context_budget,
    record_tool_outcome_with_risk, AgentRuntimeConfig, WorkerTurnPhase, WorkerTurnPolicy,
};
pub(crate) use agent_runtime::{
    bounded_max_output_tokens, observation_from_agent_tool_result, observation_from_tool_result,
    resume_agent_loop_from_messages, sanitize_assistant_content, start_agent_loop,
    start_agent_loop_with_history, AgentAdvance, AgentFailure, AgentFailureClass, AgentKernel,
    AgentLoopAppendTransaction, AgentRunControl, AgentTaskStateSnapshot, AgentToolRequest,
    AgentTurnPreparationError, ContextGovernorReport, ResultQuality, RunBudget,
    RunContinuationDirective, RunControlSnapshot, RunResourceSnapshot, RunStageClass,
    RunStopReason, WorkspaceVerificationPolicy, MAX_IDENTICAL_TOOL_FAILURES,
};
#[rustfmt::skip]
pub(crate) use agent_skills::{
    SkillCatalog, SkillRecord,
};
#[cfg(test)]
pub(crate) use agent_core::RoutingOutcome;
#[cfg(test)]
pub(crate) use agent_core::TaskClass;
pub(crate) use agent_core::{
    parse_policy, role_label, sha256_hex, AgentPolicy, OrchestrationPolicy,
    WorkspaceRetrievalChannel, WorkspaceRetrievalPlan,
};
pub(crate) use agent_core::{ConductorExecutionContract, MemoryRecallPolicy};
pub(crate) use agent_storage::{
    EventStore, PermissionAuditRecord, PermissionStore, SqliteStore, StorageError,
};
pub(crate) use base64::Engine;
#[cfg(test)]
pub(crate) use model_provider::ModelError;
pub(crate) use model_provider::{
    EmbeddingRequest, ModelCallMode, ModelRequest, ModelResponse, OpenAiCompatibleConfig,
    OpenAiCompatibleImageConfig, OpenAiCompatibleImageProvider, OpenAiCompatibleProvider,
    PreparedStreamingModelRequest, StreamingModelProvider, MODEL_REQUEST_CANCELLED,
};
#[cfg(target_os = "macos")]
pub(crate) use objc2_app_kit::{NSAutoresizingMaskOptions, NSView, NSWindow, NSWindowButton};
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use std::collections::{BTreeMap, BTreeSet};
pub(crate) use std::ffi::OsStr;
pub(crate) use std::fs;
pub(crate) use std::io::Write;
#[cfg(unix)]
pub(crate) use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
pub(crate) use std::sync::{Arc, Mutex};
pub(crate) use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
pub(crate) use tauri::Manager;
pub(crate) use tools::{
    ImageGenerationConfig, ProcessManager, ToolExecutionControl, ToolExposureIntent, ToolRegistry,
    WebSearchConfig,
};

pub(crate) use crate::agent_resource_snapshot::AgentResourceCheckpoint;
pub(crate) use crate::collaboration_service::{
    AgentCollaboration, CollaborationCompletion, COLLABORATION_STEER_INTERRUPTED,
};
pub(crate) use crate::permission_service::{
    agent_session_permission_granted, pending_agent_permissions_for_run, permission_decision_label,
    permission_decision_past_tense, permission_risk_label,
};
pub(crate) use crate::queue_service::{
    apply_queue_event, is_agent_queue_event, pending_queued_agent_messages,
    PendingQueuedAgentMessage, QueuedAgentMessageActionReceipt, QueuedAgentMessagePayload,
    QueuedAgentMessageReceipt, QueuedAgentMessageView,
};
pub(crate) use crate::runtime_values::truncate_for_collaboration;
pub(crate) use crate::schedule::{
    initial_next_run_at_ms, next_occurrence_after_ms, normalized_weekly_days,
    timestamp_ms_from_local, ScheduleCadence, ScheduleConfig, ScheduleRecord, ScheduleRunRecord,
};
pub(crate) use crate::session_projection::{
    agent_session_audits, agent_state_from_read_model, empty_agent_state_for_session,
    load_agent_session_read_model, load_agent_session_read_model_snapshot,
};
pub(crate) use crate::tool_runtime_service::{
    completed_exact_tool_result, completed_tool_result, failed_tool_result, finalize_tool_result,
    tool_input_fingerprint, tool_invocation_context, tool_invocation_event_metadata,
};
pub(crate) use agent_application::{AgentRunEvent, AgentRunStatus};
pub(crate) use agent_core::{permission_can_allow_session, permission_capability_matches};
pub(crate) use agent_runtime::{
    exhausted_model_transport_error_stop_reason, model_response_checkpoint_evidence,
    model_transport_retry_delay, ModelStreamProgress,
};
