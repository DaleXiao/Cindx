use agent_harness::ExclusiveKeyRegistry;
use std::{
    sync::{atomic::AtomicU64, OnceLock},
    time::Duration,
};

pub(crate) const PHASE3_TASK_ID: &str = "phase-3-demo";
pub(crate) const PHASE4_TASK_ID: &str = "phase-4-demo";
pub(crate) const PHASE5_TASK_ID: &str = "phase-5-tools";
pub(crate) const PHASE6_TASK_ID: &str = "phase-6-orchestration";
pub(crate) const PHASE7_TASK_ID: &str = "phase-7-rag";
pub(crate) const PHASE8_TASK_ID: &str = "phase-8-browser";
pub(crate) const PHASE15_TASK_ID: &str = "phase-15-context";
pub(crate) const PHASE16_TASK_ID: &str = "phase-16-agent-loop";
pub(crate) const MAX_ATTACHMENT_FILES: usize = 10;
pub(crate) const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
pub(crate) const MAX_ATTACHMENT_TOTAL_BYTES: usize = 50 * 1024 * 1024;
pub(crate) const PROJECT_INSTRUCTIONS_MAX_FILES: usize = 8;
pub(crate) const PROJECT_INSTRUCTIONS_MAX_FILE_BYTES: usize = 8 * 1024;
pub(crate) const PROJECT_INSTRUCTIONS_MAX_TOTAL_BYTES: usize = 16 * 1024;
pub(crate) const PROJECT_INSTRUCTIONS_MAX_ADDITIONAL_GLOBS: usize = 8;
pub(crate) const AGENT_MAX_OUTPUT_TOKENS: u64 = 32_768;
pub(crate) const MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS: usize = 4;
pub(crate) const COLLABORATION_MAX_OUTPUT_TOKENS: u64 = 4_096;
pub(crate) const ADAPTIVE_QUALITY_PASS_SCORE: f32 = 0.72;
pub(crate) const BACKGROUND_WORK_IDLE_GRACE_MS: u64 = 30_000;
pub(crate) const AGENT_SESSION_READ_MODEL_NAMESPACE: &str = "agent-session-v2";
pub(crate) const AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE: &str = "agent-runtime-snapshot-v1";
pub(crate) const AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE: &str = "agent-resource-snapshot-v1";
pub(crate) const AGENT_MEMORY_READ_MODEL_NAMESPACE: &str = "agent-memory-v3";
pub(crate) const LEGACY_AGENT_MEMORY_READ_MODEL_NAMESPACE: &str = "agent-memory-v2";
pub(crate) const AGENT_MEMORY_MAX_RECORDS: usize = 256;
pub(crate) const AGENT_MEMORY_RECALL_LIMIT: usize = 6;
pub(crate) const MEMORY_VECTOR_MANIFEST_SCHEMA: &str = "cindx.memory-vector.v1";
pub(crate) const MEMORY_VECTOR_FALLBACK_RETRY_MS: u64 = 5 * 60 * 1_000;
pub(crate) const ROUTING_TELEMETRY_READ_MODEL_NAMESPACE: &str = "routing-telemetry-v5";
pub(crate) const LEGACY_ROUTING_TELEMETRY_READ_MODEL_NAMESPACE: &str = "routing-telemetry-v4";
pub(crate) const ROUTING_TELEMETRY_READ_MODEL_KEY: &str = "global";
pub(crate) const ROUTING_TELEMETRY_MAX_RUNS: usize = 2_048;
pub(crate) const PROMPT_EVOLUTION_READ_MODEL_NAMESPACE: &str = "prompt-evolution-v2";
pub(crate) const PROMPT_EVOLUTION_READ_MODEL_KEY: &str = "global";
pub(crate) const PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1: &str =
    "cindx.prompt-distillation-canary-lease.v1";
pub(crate) const AGENT_HISTORY_INITIAL_PAGE_SIZE: usize = 120;
pub(crate) const AGENT_HISTORY_MAX_PAGE_SIZE: usize = 600;
pub(crate) const AGENT_HISTORY_MAX_TOOL_METADATA_BYTES: usize = 512 * 1024;
pub(crate) const PERSISTED_TOOL_EVENT_METADATA_VALUE_LIMIT: usize = 64 * 1024;
pub(crate) const PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES: usize = 16 * 1024;
pub(crate) const CONTEXT_RESTORE_MAX_CHARS: usize = 32_000;
pub(crate) const CONTEXT_MEMORY_MAX_ITEMS: usize = 12;
pub(crate) const CONTEXT_CHECKPOINT_MANIFEST_SCHEMA: &str = "cindx.context-checkpoint-coverage.v1";
pub(crate) const CONTEXT_COMPACTION_VERSION: &str = "hybrid_v4_prefix_events";
pub(crate) const WORKSPACE_KNOWLEDGE_CACHE_TTL: Duration = Duration::from_secs(30);
pub(crate) const WORKSPACE_KNOWLEDGE_CACHE_MAX_ENTRIES: usize = 4;
pub(crate) const SCHEDULE_POLL_INTERVAL: Duration = Duration::from_secs(10);
pub(crate) const SCHEDULE_MISSED_GRACE_MS: u64 = 90_000;
pub(crate) const SCHEDULE_DISPATCH_RETRY_MS: u64 = 60_000;
pub(crate) const SCHEDULE_MAX_DISPATCH_ATTEMPTS: u32 = 3;
pub(crate) const SCHEDULE_MAX_NAME_CHARS: usize = 80;
pub(crate) const SCHEDULE_MAX_PROMPT_CHARS: usize = 32_000;
pub(crate) const PERSONALIZATION_MAX_NAME_CHARS: usize = 80;
pub(crate) const SCHEDULE_EXECUTION_SESSION_DETAIL: &str = "schedule automation";
pub(crate) const AGENT_RECOVERY_SCHEMA: &str = "cindx.agent-recovery.v1";
pub(crate) const EVENT_REDACTION_MARKER_FILE: &str = "events-redaction-v1.complete";
pub(crate) const TOOL_EVENT_METADATA_COMPACTION_MARKER_FILE: &str =
    "events-tool-metadata-v1.complete";
pub(crate) const OPENAI_DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub(crate) const DASHSCOPE_DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-v4";
pub(crate) const LEGACY_AGENT_SYSTEM_PROMPT: &str = "You are Cindx, a desktop-first assistant. Work carefully, be direct, and ask for clarification when the task is ambiguous.";

pub(crate) static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub(crate) static MEMORY_VECTOR_REFRESH_INFLIGHT: OnceLock<ExclusiveKeyRegistry> = OnceLock::new();
pub(crate) const MAIN_WINDOW_REVEAL_FALLBACK_MS: u64 = 12_000;
#[cfg(target_os = "macos")]
pub(crate) const MACOS_TRAFFIC_LIGHT_X: f64 = 14.0;
#[cfg(target_os = "macos")]
pub(crate) const MACOS_TITLEBAR_HEIGHT: f64 = 46.0;
#[cfg(target_os = "macos")]
pub(crate) const MACOS_SIDEBAR_MATERIAL_TAG: isize = 91_376_254;
#[cfg(target_os = "macos")]
pub(crate) const MACOS_SIDEBAR_DEFAULT_WIDTH: f64 = 236.0;
#[cfg(target_os = "macos")]
pub(crate) const MACOS_TRAFFIC_LIGHT_REPAIR_DELAYS_MS: [u64; 3] = [96, 320, 900];
#[cfg(target_os = "macos")]
pub(crate) static MACOS_TRAFFIC_LIGHT_REPAIR_GENERATION: AtomicU64 = AtomicU64::new(0);
