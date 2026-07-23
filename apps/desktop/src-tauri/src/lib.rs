use agent_core::{
    Event, EventId, EventKind, Message, MessageRole, Metadata, ModelRole, PermissionDecision,
    PermissionRequest, PermissionRequestId, PermissionResolution, PermissionRisk, TaskId,
    ToolArtifact, ToolContent, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};
use agent_graph::{
    extract_graph_from_chunk, graph_direct_recall, graph_walk_recall, FileGraphStore, GraphStore,
};
use agent_mcp::{McpCatalogService, McpServerConfig, McpTransportConfig};
use agent_memory::{
    build_restore_context_pack, build_session_checkpoint_at, conversation_memory_to_markdown,
    extract_durable_memories, fuse_memory_recalls_at, memory_recalls_to_markdown,
    merge_memory_records, recall_memories_at, record_memory_observed_uses, record_memory_recalls,
    CheckpointOptions, MemoryKind, MemoryLedger, RestoreContextPack, SessionCheckpoint,
    MEMORY_LEDGER_SCHEMA,
};
use agent_rag::{
    apply_embeddings_to_index_cancellable, build_grounded_answer_prompt,
    export_lancedb_records_jsonl, index_workspace, index_workspace_cancellable,
    lancedb_index_exists, local_query_embedding, replace_lancedb_index, search_chunks_literal,
    search_lancedb_index, workspace_index_is_fresh, EmbeddingBatch, FileRagAdapter, IndexOptions,
    RagAdapter, RagChunk, RagEmbedder, RagError, RagIndex, RagIndexStats, RagSearchResult,
    RAG_INDEX_CANCELLED,
};
use agent_runtime::{
    advance_with_model_response, append_internal_instruction, append_steering_instruction,
    append_tool_observation, bounded_max_output_tokens, completion_verification_instruction,
    compose_agent_system_prompt, evidence_worker_tools,
    interaction_completion_verification_instruction, model_request_for_turn_with_context_budget,
    observation_from_tool_result, record_tool_outcome, record_tool_outcome_with_risk,
    repeated_tool_failure_count, resume_agent_loop_from_messages, sanitize_assistant_content,
    start_agent_loop, start_agent_loop_with_history, tool_invocation_from_request, AgentAdvance,
    AgentRunControl, AgentRuntimeConfig, ResultQuality, RunBudget, RunControlSnapshot,
    RunStageClass, RunStopReason,
    DEFAULT_COLLABORATION_WORKER_TURNS, MAX_COLLABORATION_WORKER_TOOL_CALLS,
    MAX_IDENTICAL_TOOL_FAILURES,
};
use agent_skills::{
    install_skill_archive as install_skill_archive_package, SkillCatalog, SkillPreference,
    SkillRecord,
};
use agent_storage::{
    EventStore, PermissionAuditRecord, PermissionStore, SqliteStore, StorageError,
};
use base64::Engine;
use model_provider::{
    EmbeddingRequest, ModelCallMode, ModelRequest, OpenAiCompatibleConfig,
    OpenAiCompatibleImageConfig, OpenAiCompatibleImageProvider, OpenAiCompatibleProvider,
    MODEL_REQUEST_CANCELLED,
};
#[cfg(test)]
use model_provider::ModelError;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView, NSWindow, NSWindowButton};
#[cfg(test)]
use orchestrator::AgentEvaluationVerifier;
use orchestrator::{
    adaptive_worker_prompt, adaptive_workflow_layers, adaptive_workflow_step_budget, default_plan,
    derive_prompt_evolution_campaign, evaluate_prompt_convergence, parse_policy,
    prompt_promotion_confidence, prompt_reflection_packets, role_label, sha256_hex, step_prompt,
    ActionableSideInformation, AgentEvaluationCaseScore, AgentEvaluationCheck,
    AgentEvaluationEvidenceSource, AgentEvaluationReflectionPacket, AgentEvaluationSplit,
    AgentEvaluationToolTrace, AgentEvaluationTrace, AgentEvaluationTraceStep,
    AgentEvaluationVerifierOutcome, ConductorExecutionContract, ConductorHarness,
    ConductorPromptGenome, ConductorRequest, ConductorRoleHints, LearnedModelRouter,
    ModelCandidate, OrchestrationPolicy,
    PromptEvaluationMode, PromptEvaluationSplit, PromptEvolutionCampaignInput,
    PromptEvolutionCampaignSnapshot, PromptEvolutionObservation, PromptInstanceParetoArchive,
    PromptParetoArchive, PromptPromotionConfidence, PromptRetryPolicy, PromptStepCredit,
    PromptVerification, RoutingContext, RoutingDecision, RoutingOutcome, RoutingTelemetry,
    RuleBasedRouter, TaskClass, WorkflowBudget, WorkflowExecutionCheckpoint,
    WorkflowExecutionTelemetry, WorkflowPlanIr, WorkflowSearchTeacher, WorkflowStepStatus,
    WorkflowToolPolicy, WorkflowTopologyPrior, AGENT_EVALUATION_TRACE_SCHEMA,
    CONDUCTOR_MAX_ATTEMPTS, MAX_ADAPTIVE_WORKFLOW_AGENTS, WORKFLOW_CHECKPOINT_SCHEMA,
    WORKFLOW_IR_SCHEMA,
};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
use tools::{
    prompt_requests_image_generation, ImageGenerationConfig, ToolExecutionControl, ToolRegistry,
    WebSearchConfig,
};

mod adaptive_collaboration_runtime;
mod agent_collaboration_runtime;
mod agent_commands;
mod agent_loop_runtime;
mod agent_loop_service;
mod agent_query_commands;
mod agent_read_model;
mod agent_recovery_service;
mod app_bootstrap;
mod app_state;
mod collaboration_execution;
mod collaboration_models;
mod collaboration_service;
mod configuration_models;
mod event_projection;
mod integration_commands;
mod knowledge_commands;
mod knowledge_runtime;
mod native_commands;
mod orchestration_commands;
mod parallel_execution;
mod permission_service;
mod persistence_runtime;
mod platform_runtime;
mod project_commands;
mod prompt_evaluation_runtime;
mod prompt_evidence_runtime;
mod prompt_evolution_models;
mod prompt_evolution_read_model;
mod prompt_evolution_runtime;
mod prompt_mutation_runtime;
mod prompt_pairwise_runtime;
mod queue_service;
mod routing_learning_runtime;
mod run_lifecycle;
mod runtime_constants;
mod schedule;
mod schedule_commands;
mod session_context_service;
mod session_projection;
mod session_title_service;
mod settings_commands;
mod suspended_run_runtime;
mod tool_commands;
mod tool_execution;
mod tool_runtime_service;
mod view_models;
mod workflow_checkpoint_runtime;
mod workflow_routing_runtime;

use adaptive_collaboration_runtime::*;
use agent_collaboration_runtime::*;
use agent_commands::*;
use agent_loop_runtime::*;
use agent_query_commands::*;
use agent_read_model::*;
pub use app_bootstrap::run;
use app_bootstrap::QuitConfirmation;
use app_state::*;
use collaboration_execution::*;
use collaboration_models::*;
use configuration_models::*;
use event_projection::*;
use integration_commands::*;
use knowledge_commands::*;
use knowledge_runtime::*;
use native_commands::*;
use orchestration_commands::*;
use persistence_runtime::*;
use platform_runtime::*;
use project_commands::*;
use prompt_evaluation_runtime::*;
use prompt_evidence_runtime::*;
use prompt_evolution_models::*;
use prompt_evolution_read_model::*;
use prompt_evolution_runtime::*;
use prompt_mutation_runtime::*;
use prompt_pairwise_runtime::*;
use routing_learning_runtime::*;
use runtime_constants::*;
use schedule_commands::*;
use settings_commands::*;
use suspended_run_runtime::*;
use tool_commands::*;
use tool_execution::*;
use view_models::*;
use workflow_checkpoint_runtime::*;
use workflow_routing_runtime::*;

use tool_runtime_service::{
    completed_tool_result, failed_tool_result, finalize_tool_result, tool_input_fingerprint,
    tool_invocation_context, tool_invocation_event_metadata,
};

use agent_application::{
    artifact_manifest_message, project_agent_artifacts as agent_output_artifacts_from_events,
    project_session_lifecycle, AgentOutputArtifact as AgentOutputArtifactView,
    SessionLifecycleInput, SessionTitleState,
};
use agent_loop_service::{
    exhausted_model_transport_error_stop_reason, model_response_checkpoint_evidence,
    model_transport_retry_delay, ModelStreamProgress,
};
use agent_recovery_service::*;
use collaboration_service::{
    adaptive_model_role, adaptive_stage_metadata, build_collaboration_arbiter_prompt,
    build_collaboration_candidate_prompt, collaboration_agent_budget,
    collaboration_context_for_genome, collaboration_fallback_models, collaboration_recent_context,
    collaboration_recovery_evidence, collaboration_step_result,
    collaboration_worker_runtime_turn_limit, effective_workflow_model_turn_budget,
    effective_workflow_step_attempt_budget, merge_collaboration_evidence,
    prepare_collaboration_worker_turn, truncate_for_collaboration, workflow_role_coverage,
    AdaptiveCollaborationSpec, AgentCollaboration, CollaborationCompletion, CollaborationEvidence,
    WORKFLOW_RESUMABLE_ERROR_PREFIX, WORKFLOW_SAFETY_ERROR_PREFIX,
};
use parallel_execution::{
    run_model_jobs_ordered, run_model_jobs_until_quorum, CancellableParallelJob, ParallelJob,
};
use permission_service::{
    agent_session_permission_granted, pending_agent_permissions_for_run, permission_decision_label,
    permission_decision_past_tense, permission_risk_label,
};
use queue_service::{
    apply_queue_event, is_agent_queue_event, pending_queued_agent_messages,
    PendingQueuedAgentMessage, QueuedAgentMessageActionReceipt, QueuedAgentMessagePayload,
    QueuedAgentMessageReceipt, QueuedAgentMessageView,
};
use run_lifecycle::{AgentRunEvent, AgentRunStatus, ExclusiveKeyLease, RegisteredRunControl};
use schedule::{
    initial_next_run_at_ms, next_occurrence_after_ms, normalized_weekly_days,
    timestamp_ms_from_local, ScheduleCadence, ScheduleConfig, ScheduleRecord, ScheduleRunRecord,
};
use session_context_service::*;
use session_projection::{
    agent_session_audits, agent_state_from_read_model, empty_agent_state_for_session,
    load_agent_session_read_model, load_agent_session_read_model_snapshot,
};
use session_title_service::*;

#[cfg(test)]
mod external_effect_eval_tests;
#[cfg(test)]
mod tests;
