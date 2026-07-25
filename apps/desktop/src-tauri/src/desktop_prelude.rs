pub(crate) use agent_application::{
    artifact_manifest_message, project_agent_artifacts as agent_output_artifacts_from_events,
    project_session_lifecycle, AgentOutputArtifact as AgentOutputArtifactView,
    SessionLifecycleInput, SessionTitleState,
};
pub(crate) use agent_core::{
    Event, EventId, EventKind, Message, MessageRole, Metadata, ModelRole, PermissionDecision,
    PermissionRequest, PermissionRequestId, PermissionResolution, PermissionRisk, TaskId,
    ToolArtifact, ToolContent, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};
pub(crate) use agent_graph::{
    extract_graph_from_chunk, graph_direct_recall, graph_walk_recall, FileGraphStore, GraphStore,
};
pub(crate) use agent_mcp::{McpCatalogService, McpServerConfig, McpTransportConfig};
pub(crate) use agent_memory::{
    build_restore_context_pack, build_session_checkpoint_at, conversation_memory_to_markdown,
    extract_durable_memories, fuse_memory_recalls_at, memory_recalls_to_markdown,
    merge_memory_records, recall_memories_at, record_memory_observed_uses, record_memory_recalls,
    CheckpointOptions, MemoryKind, MemoryLedger, RestoreContextPack, SessionCheckpoint,
    MEMORY_LEDGER_SCHEMA,
};
pub(crate) use agent_rag::{
    apply_embeddings_to_index_cancellable, build_grounded_answer_prompt,
    export_lancedb_records_jsonl,
    fuse_retrieval_channels_for_query as fuse_rag_retrieval_channels_for_query, index_workspace,
    index_workspace_cancellable, lancedb_index_exists, local_query_embedding,
    merge_retrieval_channel, replace_lancedb_index, retrieval_ranges_overlap,
    search_chunks_literal, search_lancedb_index, workspace_index_is_fresh, EmbeddingBatch,
    FileRagAdapter, IndexOptions, RagAdapter, RagChunk, RagEmbedder, RagError, RagIndex,
    RagIndexStats, RagSearchResult, RetrievalChannelOutcome, RAG_INDEX_CANCELLED,
};
#[cfg(test)]
pub(crate) use agent_runtime::{
    advance_with_model_response, model_request_for_turn_with_context_budget,
    record_tool_outcome_with_risk, AgentRuntimeConfig, WorkerTurnPhase, WorkerTurnPolicy,
};
pub(crate) use agent_runtime::{
    bounded_max_output_tokens, compose_agent_system_prompt, ensure_terminal_commit_instruction,
    evidence_worker_tools, observation_from_tool_result, resume_agent_loop_from_messages,
    sanitize_assistant_content, start_agent_loop, start_agent_loop_with_history, AgentAdvance,
    AgentFailure, AgentFailureClass, AgentKernel, AgentRecoveryAction, AgentRunControl,
    AgentTaskStateSnapshot, AgentToolRequest, ContextGovernorReport, IsolatedWorkerRuntime,
    ResultQuality, RunBudget, RunContinuationDirective, RunControlSnapshot, RunStageClass,
    RunStopReason, WorkerAdvance, WorkerToolAdmission, DEFAULT_COLLABORATION_WORKER_TURNS,
    MAX_COLLABORATION_WORKER_TOOL_CALLS, MAX_IDENTICAL_TOOL_FAILURES,
};
pub(crate) use agent_skills::{
    install_skill_archive as install_skill_archive_package, SkillCatalog, SkillPreference,
    SkillRecord,
};
pub(crate) use agent_storage::{
    EventStore, PermissionAuditRecord, PermissionStore, SqliteStore, StorageError,
};
pub(crate) use base64::Engine;
#[cfg(test)]
pub(crate) use model_provider::ModelError;
pub(crate) use model_provider::{
    EmbeddingRequest, ModelCallMode, ModelRequest, ModelResponse, OpenAiCompatibleConfig,
    OpenAiCompatibleImageConfig, OpenAiCompatibleImageProvider, OpenAiCompatibleProvider,
    StreamingModelProvider, MODEL_REQUEST_CANCELLED,
};
#[cfg(target_os = "macos")]
pub(crate) use objc2_app_kit::{NSAutoresizingMaskOptions, NSView, NSWindow, NSWindowButton};
#[cfg(test)]
pub(crate) use orchestrator::AgentEvaluationVerifier;
pub(crate) use orchestrator::{
    adaptive_worker_prompt, adaptive_workflow_layers, adaptive_workflow_step_budget,
    decide_uplift_gate, default_plan, derive_prompt_evolution_campaign,
    evaluate_prompt_convergence, evaluate_prompt_promotion_gate, parse_policy,
    prompt_reflection_packets, role_label, sha256_hex, step_prompt, ActionableSideInformation,
    AgentEvaluationCaseScore, AgentEvaluationCheck, AgentEvaluationEvidenceSource,
    AgentEvaluationReflectionPacket, AgentEvaluationSplit, AgentEvaluationToolTrace,
    AgentEvaluationTrace, AgentEvaluationTraceStep, AgentEvaluationVerifierOutcome,
    AnytimeCandidate, AnytimeCandidateKind, AnytimeCandidateState, AnytimeController,
    AnytimeControllerConfig, AnytimeControllerSnapshot, AnytimeDecision, AnytimeVerdict,
    ConductorExecutionContract, ConductorHarness, ConductorPromptGenome, ConductorRequest,
    ConductorRoleHints, ConductorStopPolicy, LearnedModelRouter, ModelCandidate,
    OrchestrationPolicy, PromptEvaluationMode, PromptEvaluationSplit, PromptEvolutionCampaignInput,
    PromptEvolutionCampaignSnapshot, PromptEvolutionObservation, PromptInstanceParetoArchive,
    PromptParetoArchive, PromptPromotionConfidence, PromptPromotionGateConfig, PromptRetryPolicy,
    PromptStepCredit, PromptVerification, RoutingContext, RoutingDecision, RoutingOutcome,
    RoutingTelemetry, RuleBasedRouter, TaskClass, UpliftGap, UpliftGateDecision, UpliftGateInput,
    WorkflowBudget, WorkflowExecutionCheckpoint, WorkflowExecutionTelemetry, WorkflowPlanIr,
    WorkflowSearchTeacher, WorkflowStepStatus, WorkflowToolPolicy, WorkflowTopologyPrior,
    WorkflowVerificationState, AGENT_EVALUATION_TRACE_SCHEMA, CONDUCTOR_MAX_ATTEMPTS,
    MAX_ADAPTIVE_WORKFLOW_AGENTS, WORKFLOW_CHECKPOINT_SCHEMA, WORKFLOW_IR_SCHEMA,
};
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use std::cmp::Reverse;
pub(crate) use std::collections::{BTreeMap, BTreeSet};
pub(crate) use std::ffi::OsStr;
pub(crate) use std::fs;
pub(crate) use std::io::Write;
#[cfg(unix)]
pub(crate) use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
pub(crate) use std::sync::{Arc, Mutex, OnceLock};
pub(crate) use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
pub(crate) use tauri::{Emitter, Manager};
pub(crate) use tools::{
    prompt_requests_image_generation, ImageGenerationConfig, ToolExecutionControl, ToolRegistry,
    WebSearchConfig,
};

pub(crate) use crate::agent_loop_service::{
    exhausted_model_transport_error_stop_reason, model_response_checkpoint_evidence,
    model_transport_retry_delay, ModelStreamProgress,
};
pub(crate) use crate::collaboration_service::{
    adaptive_model_role, adaptive_stage_metadata, build_collaboration_arbiter_prompt,
    build_collaboration_candidate_prompt, collaboration_agent_budget,
    collaboration_context_for_genome, collaboration_fallback_models, collaboration_recent_context,
    collaboration_recovery_evidence, collaboration_step_result,
    effective_workflow_model_turn_budget, effective_workflow_step_attempt_budget,
    merge_collaboration_evidence, truncate_for_collaboration, workflow_role_coverage,
    AdaptiveCollaborationOutcome, AdaptiveCollaborationSpec, AgentCollaboration,
    CollaborationCompletion, CollaborationEvidence, COLLABORATION_STEER_INTERRUPTED,
    WORKFLOW_RESUMABLE_ERROR_PREFIX, WORKFLOW_SAFETY_ERROR_PREFIX,
};
pub(crate) use crate::parallel_execution::{
    model_job_supervisor, run_model_jobs_until_anytime_quorum_interruptible,
    run_model_jobs_until_quorum_interruptible, AnytimeQuorumPolicy, CancellableParallelJob,
    InterruptibleQuorumPolicy, ParallelJobCompletion, ParallelJobSupervisor,
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
pub(crate) use crate::run_lifecycle::{
    AgentRunEvent, AgentRunStatus, ExclusiveKeyLease, RegisteredRunControl,
};
pub(crate) use crate::schedule::{
    initial_next_run_at_ms, next_occurrence_after_ms, normalized_weekly_days,
    timestamp_ms_from_local, ScheduleCadence, ScheduleConfig, ScheduleRecord, ScheduleRunRecord,
};
pub(crate) use crate::session_projection::{
    agent_session_audits, agent_state_from_read_model, empty_agent_state_for_session,
    load_agent_session_read_model, load_agent_session_read_model_snapshot,
};
pub(crate) use crate::tool_runtime_service::{
    completed_tool_result, failed_tool_result, finalize_tool_result, tool_input_fingerprint,
    tool_invocation_context, tool_invocation_event_metadata,
};
