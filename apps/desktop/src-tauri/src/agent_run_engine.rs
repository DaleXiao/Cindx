#[path = "agent_execution_provider_runtime.rs"]
mod execution_providers;

use crate::agent_loop_runtime::{
    execute_agent_loop_epoch_with_provider, AgentLoopExecutionOutcome,
};
use crate::agent_preparation_runtime::prepare_agent_execution_replay;
use crate::agent_query_commands::finish_agent_run_for_control_stop_with_task_state;
use crate::agent_read_model::agent_state_with_error_in_context;
use crate::app_state::AppState;
use crate::collaboration_service::AgentCollaboration;
use crate::configuration_models::ProviderConfig;
use crate::view_models::AgentState;
use agent_application::{execute_agent_run, AgentRunEpoch, AgentRunExecutor, AgentRunPreparation};
use agent_core::AgentPolicy;
use agent_core::{Message, Metadata, ModelRole, TaskId};
use agent_runtime::{AgentLoopState, AgentRunControl};
use execution_providers::build_agent_execution_providers;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum AgentRunPreparationError {
    Finished(Box<Result<AgentState, String>>),
    ControlStop(Metadata),
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
    effort: AgentPolicy,
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
        mut prepared: PreparedAgentExecution,
        effort: AgentPolicy,
        cancellation: &Arc<AgentRunControl>,
    ) -> Result<AgentState, String> {
        prepared.runtime.generation_temperature =
            effort.generation_temperature().map(str::to_string);
        prepared.runtime.reasoning_effort = Some(effort.label().to_string());
        let mut executor = DesktopAgentRunExecutor {
            app: self.app,
            state: self.state,
            config,
            workspace_root,
            effort,
            cancellation,
            base_run_context: prepared.base_run_context.clone(),
        };
        let task_id = prepared.runtime.task_id.clone();
        let run_context = prepared.base_run_context.clone();
        let result = execute_agent_run(&mut executor, prepared);
        if !matches!(&result, Ok(state) if state.status == "waiting_for_permission") {
            self.state
                .process_manager
                .shutdown_run(&task_id, &run_context);
        }
        result
    }
}

struct AgentRunReprepare {
    runtime: AgentLoopState,
    prompt: String,
}

struct DesktopAgentRunExecutor<'a, 'state> {
    app: &'a tauri::AppHandle,
    state: &'a tauri::State<'state, AppState>,
    config: &'a ProviderConfig,
    workspace_root: &'a Path,
    effort: AgentPolicy,
    cancellation: &'a Arc<AgentRunControl>,
    base_run_context: Metadata,
}

impl AgentRunExecutor for DesktopAgentRunExecutor<'_, '_> {
    type Prepared = PreparedAgentExecution;
    type Reprepare = AgentRunReprepare;
    type Output = AgentState;
    type Error = String;

    fn execute_epoch(
        &mut self,
        prepared: Self::Prepared,
    ) -> Result<AgentRunEpoch<Self::Reprepare, Self::Output>, Self::Error> {
        let agent_model = effort_tier_model(self.config, self.effort.label());
        let providers =
            build_agent_execution_providers(self.config, &agent_model, self.cancellation);
        match execute_agent_loop_epoch_with_provider(
            self.app,
            self.state,
            self.config,
            self.workspace_root,
            prepared.runtime,
            prepared.prompt,
            prepared.run_context,
            prepared.collaboration.as_ref(),
            self.cancellation,
            &providers.actor,
            &providers.finalizer,
            &agent_model,
        )? {
            AgentLoopExecutionOutcome::Finished(agent_state) => {
                Ok(AgentRunEpoch::Finished(agent_state))
            }
            AgentLoopExecutionOutcome::Reprepare { runtime, prompt } => {
                Ok(AgentRunEpoch::Reprepare(AgentRunReprepare {
                    runtime,
                    prompt,
                }))
            }
        }
    }

    fn reprepare(
        &mut self,
        handoff: Self::Reprepare,
    ) -> Result<AgentRunPreparation<Self::Prepared, Self::Output>, Self::Error> {
        let task_id = handoff.runtime.task_id.clone();
        match prepare_agent_execution(
            self.app,
            self.state,
            self.config,
            &task_id,
            self.workspace_root,
            self.base_run_context.clone(),
            handoff.runtime,
            handoff.prompt,
            None,
            self.effort,
            self.cancellation,
        ) {
            Ok(prepared) => Ok(AgentRunPreparation::Prepared(prepared)),
            Err(AgentRunPreparationError::Finished(result)) => {
                (*result).map(AgentRunPreparation::Finished)
            }
            Err(AgentRunPreparationError::ControlStop(run_context)) => {
                finish_agent_run_for_control_stop_with_task_state(
                    self.app,
                    self.state,
                    &run_context,
                    self.cancellation,
                    None,
                )
                .map(AgentRunPreparation::Finished)
            }
            Err(AgentRunPreparationError::Runtime { error, run_context }) => {
                agent_state_with_error_in_context(self.state, &run_context, error)
                    .map(AgentRunPreparation::Finished)
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
    effort: AgentPolicy,
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

/// Effort-tier model selection: the actor/finalizer model is the pinned default for
/// the run's effort tier (Fast/Auto/Pro), falling back to the executor role model.
/// This is the single scheduling source; no conductor/router override is consulted.
pub(crate) fn effort_tier_model(config: &ProviderConfig, effort_label: &str) -> String {
    let pinned = config.effort_default_model(effort_label);
    if pinned.trim().is_empty() {
        config.model_for_role(&ModelRole::Executor)
    } else {
        pinned
    }
}
