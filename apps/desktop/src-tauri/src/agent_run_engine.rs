use crate::agent_preparation_runtime::prepare_agent_execution_replay;
use crate::app_state::AppState;
use crate::collaboration_service::AgentCollaboration;
use crate::configuration_models::{AgentEffort, ProviderConfig};
use agent_core::{Message, Metadata, TaskId};
use agent_runtime::{AgentLoopState, AgentRunControl};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum AgentRunPreparationError {
    ControlStop(Metadata),
    Collaboration {
        error: String,
        run_context: Metadata,
    },
    Runtime {
        error: String,
        run_context: Metadata,
    },
}

#[derive(Debug)]
pub(crate) struct PreparedAgentExecution {
    pub(crate) base_run_context: Metadata,
    pub(crate) run_context: Metadata,
    pub(crate) runtime: AgentLoopState,
    pub(crate) prompt: String,
    pub(crate) collaboration: Option<AgentCollaboration>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_execution(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: Metadata,
    runtime: AgentLoopState,
    prompt: String,
    artifact_manifest: Option<Message>,
    effort: AgentEffort,
    cancellation: &Arc<AgentRunControl>,
) -> Result<PreparedAgentExecution, AgentRunPreparationError> {
    prepare_agent_execution_replay(
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        runtime,
        prompt,
        artifact_manifest,
        effort,
        cancellation,
    )
}

pub(crate) fn runtime_preparation_error(
    run_context: &Metadata,
    error: impl Into<String>,
) -> AgentRunPreparationError {
    AgentRunPreparationError::Runtime {
        error: error.into(),
        run_context: run_context.clone(),
    }
}
