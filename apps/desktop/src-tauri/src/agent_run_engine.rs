use crate::agent_loop_runtime::{
    execute_agent_loop_epoch_with_provider, AgentLoopExecutionOutcome,
};
use crate::agent_preparation_runtime::prepare_agent_execution_replay;
use crate::agent_query_commands::finish_agent_run_for_control_stop_with_task_state;
use crate::agent_read_model::agent_state_with_error_in_context;
use crate::app_state::AppState;
use crate::collaboration_service::AgentCollaboration;
use crate::configuration_models::{agent_model_for_run, AgentEffort, ProviderConfig};
use crate::runtime_constants::AGENT_MODEL_RECOVERY_WINDOW_SECONDS;
use crate::view_models::AgentState;
use agent_core::{Message, Metadata, ModelRole, TaskId};
use agent_runtime::{AgentLoopState, AgentRunControl, RunStageClass};
use model_provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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
pub(crate) fn continue_agent_loop(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    prepared: PreparedAgentExecution,
    effort: AgentEffort,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    AgentExecutionService::new(app, state).execute_prepared(
        config,
        workspace_root,
        prepared,
        effort,
        cancellation,
    )
}

pub(crate) struct AgentExecutionService<'app, 'state> {
    app: &'app tauri::AppHandle,
    state: &'app tauri::State<'state, AppState>,
}

impl<'app, 'state> AgentExecutionService<'app, 'state> {
    pub(crate) fn new(
        app: &'app tauri::AppHandle,
        state: &'app tauri::State<'state, AppState>,
    ) -> Self {
        Self { app, state }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_prepared(
        &self,
        config: &ProviderConfig,
        workspace_root: &Path,
        prepared: PreparedAgentExecution,
        effort: AgentEffort,
        cancellation: &Arc<AgentRunControl>,
    ) -> Result<AgentState, String> {
        let base_run_context = prepared.base_run_context;
        let mut run_context = prepared.run_context;
        let mut runtime = prepared.runtime;
        let mut prompt = prepared.prompt;
        let mut collaboration = prepared.collaboration;

        loop {
            let agent_model = agent_model_for_run(config, &run_context);
            let provider_timeout = if collaboration.is_some() {
                cancellation
                    .stage_model_call_timeout_with_recovery(
                        RunStageClass::Finalizer,
                        1,
                        Duration::from_secs(AGENT_MODEL_RECOVERY_WINDOW_SECONDS),
                    )
                    .as_secs()
                    .max(1)
            } else {
                cancellation.model_call_timeout_seconds()
            };
            let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: config.base_url.clone(),
                api_key: config.api_key.clone(),
                model: agent_model.clone(),
                embedding_model: config.model_for_role(&ModelRole::Embedder),
                timeout_seconds: provider_timeout,
            });

            match execute_agent_loop_epoch_with_provider(
                self.app,
                self.state,
                config,
                workspace_root,
                runtime,
                prompt,
                run_context,
                collaboration.as_ref(),
                cancellation,
                &provider,
                &agent_model,
            )? {
                AgentLoopExecutionOutcome::Finished(agent_state) => return Ok(agent_state),
                AgentLoopExecutionOutcome::Reprepare {
                    runtime: steered_runtime,
                    prompt: steered_prompt,
                } => {
                    let task_id = steered_runtime.task_id.clone();
                    let next = prepare_agent_execution(
                        self.app,
                        self.state,
                        config,
                        &task_id,
                        workspace_root,
                        base_run_context.clone(),
                        steered_runtime,
                        steered_prompt,
                        None,
                        effort,
                        cancellation,
                    );
                    let next = match next {
                        Ok(prepared) => prepared,
                        Err(AgentRunPreparationError::ControlStop(run_context)) => {
                            return finish_agent_run_for_control_stop_with_task_state(
                                self.app,
                                self.state,
                                &run_context,
                                cancellation,
                                None,
                            )
                        }
                        Err(AgentRunPreparationError::Collaboration { error, run_context }) => {
                            return agent_state_with_error_in_context(
                                self.state,
                                &run_context,
                                format!("Collaboration failed: {error}"),
                            )
                        }
                        Err(AgentRunPreparationError::Runtime { error, run_context }) => {
                            return agent_state_with_error_in_context(
                                self.state,
                                &run_context,
                                error,
                            )
                        }
                    };
                    run_context = next.run_context;
                    runtime = next.runtime;
                    prompt = next.prompt;
                    collaboration = next.collaboration;
                }
            }
        }
    }
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
