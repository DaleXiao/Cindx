use crate::desktop_prelude::*;
use crate::rag_operation_runtime::RagOperationControl;
use crate::{
    attachment_upload_batches::AttachmentUploadBatches,
    collaboration_service::AgentCollaboration,
    conductor_health_runtime::ConductorHealthLedger,
    configuration_models::{ProjectSessionConfig, ProviderConfig, SidecarConfig, WorkspaceConfig},
    schedule::ScheduleConfig,
};

pub(crate) struct AppState {
    pub(crate) store: Mutex<SqliteStore>,
    pub(crate) attachment_upload_batches: Mutex<AttachmentUploadBatches>,
    pub(crate) manual_tool_execution_gate: Mutex<()>,
    pub(crate) provider_config: Mutex<ProviderConfig>,
    pub(crate) provider_config_update: Mutex<()>,
    pub(crate) workspace_config: Mutex<WorkspaceConfig>,
    pub(crate) sidecar_config: Mutex<SidecarConfig>,
    pub(crate) web_search_config: Mutex<WebSearchConfig>,
    pub(crate) project_session_config: Mutex<ProjectSessionConfig>,
    pub(crate) session_lifecycle_gate: Mutex<()>,
    pub(crate) schedule_config: Mutex<ScheduleConfig>,
    pub(crate) schedule_last_error: Mutex<Option<String>>,
    pub(crate) mcp_catalog: Mutex<McpCatalogService>,
    pub(crate) suspended_agent_runs: Mutex<BTreeMap<String, SuspendedAgentRun>>,
    pub(crate) session_output_cache: Mutex<BTreeMap<String, SessionOutputCacheEntry>>,
    pub(crate) agent_run_controls: Mutex<BTreeMap<String, Arc<AgentRunControl>>>,
    pub(crate) prompt_evaluation_controls: Mutex<BTreeMap<String, Arc<AgentRunControl>>>,
    pub(crate) rag_operation_controls: Mutex<BTreeMap<String, Arc<RagOperationControl>>>,
    pub(crate) queue_dispatching_sessions: Mutex<BTreeSet<String>>,
    pub(crate) session_title_refinement_sessions: Mutex<BTreeSet<String>>,
    pub(crate) workspace_knowledge_cache: Mutex<BTreeMap<String, WorkspaceKnowledgeCacheEntry>>,
    pub(crate) tool_registry_cache: Mutex<ToolRegistryCache>,
    pub(crate) conductor_health: Mutex<ConductorHealthLedger>,
    pub(crate) tool_registry_generation: AtomicU64,
    pub(crate) allow_exit: AtomicBool,
    pub(crate) quit_prompt_active: AtomicBool,
}

pub(crate) const TOOL_REGISTRY_CACHE_LIMIT: usize = 8;

#[derive(Clone)]
pub(crate) struct ToolRegistryCacheEntry {
    pub(crate) generation: u64,
    pub(crate) registry: ToolRegistry,
}

#[derive(Default)]
pub(crate) struct ToolRegistryCache {
    pub(crate) entries: BTreeMap<PathBuf, ToolRegistryCacheEntry>,
}

impl ToolRegistryCache {
    pub(crate) fn get(&self, workspace_root: &Path, generation: u64) -> Option<ToolRegistry> {
        self.entries
            .get(workspace_root)
            .filter(|entry| entry.generation == generation)
            .map(|entry| entry.registry.clone())
    }

    pub(crate) fn insert(
        &mut self,
        workspace_root: PathBuf,
        generation: u64,
        registry: ToolRegistry,
    ) {
        if !self.entries.contains_key(&workspace_root)
            && self.entries.len() >= TOOL_REGISTRY_CACHE_LIMIT
        {
            if let Some(oldest_key) = self.entries.keys().next().cloned() {
                self.entries.remove(&oldest_key);
            }
        }
        self.entries.insert(
            workspace_root,
            ToolRegistryCacheEntry {
                generation,
                registry,
            },
        );
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceKnowledgeCacheEntry {
    pub(crate) adapter: FileRagAdapter,
    pub(crate) graph_store: Option<Arc<FileGraphStore>>,
    pub(crate) validated_at: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct SuspendedAgentRun {
    pub(crate) runtime: agent_runtime::AgentLoopState,
    pub(crate) prompt: String,
    pub(crate) run_context: Metadata,
    pub(crate) workspace_root: PathBuf,
    pub(crate) collaboration: Option<AgentCollaboration>,
    pub(crate) run_control: RunControlSnapshot,
    pub(crate) last_touched_at_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionOutputCacheEntry {
    pub(crate) event_count: u64,
    pub(crate) latest_sequence: u64,
    pub(crate) outputs: Vec<AgentOutputArtifactView>,
    pub(crate) last_accessed_at_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRecoveryEnvelope {
    pub(crate) schema: String,
    pub(crate) resume_key: String,
    pub(crate) project_id: Option<String>,
    pub(crate) session_id: String,
    pub(crate) source_run_id: String,
    pub(crate) user_turn_sequence: u64,
    pub(crate) prompt_fingerprint: String,
    pub(crate) effort: String,
    pub(crate) policy: String,
    pub(crate) queue_id: Option<String>,
    pub(crate) workflow_resume_key: Option<String>,
    pub(crate) state: String,
    pub(crate) reason: String,
    pub(crate) attempts: u32,
    pub(crate) model_calls: usize,
    pub(crate) tool_calls: usize,
    #[serde(default)]
    pub(crate) material_checkpoints: usize,
    #[serde(default)]
    pub(crate) observations: usize,
    #[serde(default)]
    pub(crate) budget_extensions: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) task_state: Option<AgentTaskStateSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resource_snapshot: Option<RunResourceSnapshot>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedToolObservation {
    pub(crate) call_id: agent_core::ToolCallId,
    pub(crate) tool_name: String,
    pub(crate) input_json: String,
    pub(crate) status: ToolOutcomeStatus,
    pub(crate) observation: String,
    pub(crate) image_paths: Vec<String>,
}
