use crate::agent_collaboration_runtime::{
    append_agent_collaboration_context, prepare_agent_collaboration_or_degrade,
};
use crate::agent_query_commands::{agent_run_should_stop, append_agent_progress_event};
use crate::agent_strategy_runtime::plan_agent_run;
use crate::app_state::AppState;
use crate::collaboration_service::AgentCollaboration;
use crate::configuration_models::{AgentEffort, ProviderConfig};
use crate::memory_runtime::{
    append_prepared_memory_context, append_skill_context_for_run, prepare_run_knowledge_contexts,
};
use crate::session_context_service::prepare_session_history_context;
use agent_core::{Message, MessageRole, Metadata, TaskId};
use agent_runtime::AgentRunControl;
use model_provider::MODEL_REQUEST_CANCELLED;
use orchestrator::OrchestrationPolicy;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum AgentRunPreparationError {
    ControlStop(Metadata),
    Collaboration {
        error: String,
        run_context: Metadata,
    },
    Runtime(String),
}

#[derive(Debug)]
pub(crate) struct PreparedAgentExecution {
    pub(crate) run_context: Metadata,
    pub(crate) history: Vec<Message>,
    pub(crate) collaboration: Option<AgentCollaboration>,
}

fn append_single_model_policy_guidance(
    history: &mut Vec<Message>,
    policy: &OrchestrationPolicy,
) {
    if *policy != OrchestrationPolicy::PlanExecuteReview {
        return;
    }
    history.push(Message {
        role: MessageRole::System,
        content: "Use a single-model plan-execute-review loop for this request: form a concise plan, execute only the required tools, verify the result against evidence, then answer. Do not expose private chain-of-thought; report only decisions, actions, and verified results.".to_string(),
        metadata: [
            ("internal".to_string(), "true".to_string()),
            (
                "kind".to_string(),
                "single_model_policy_guidance".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_execution(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    mut run_context: Metadata,
    prompt: &str,
    history: Vec<Message>,
    artifact_manifest: Option<Message>,
    effort: AgentEffort,
    cancellation: &Arc<AgentRunControl>,
) -> Result<PreparedAgentExecution, AgentRunPreparationError> {
    let mut history = prepare_session_history_context(
        state,
        workspace_root,
        &run_context,
        history,
        config.context_window_tokens,
    )
    .map_err(|error| {
        AgentRunPreparationError::Runtime(format!("context preparation failed: {error}"))
    })?;

    cancellation.mark_progress("strategy", "Selecting the execution path");
    let plan = plan_agent_run(
        state,
        config,
        task_id,
        &mut run_context,
        prompt,
        &history,
        effort,
    )
    .map_err(|error| classify_preparation_error(error, cancellation, &run_context))?;

    if cancellation.has_pending_steer() {
        if let Some(artifact_manifest) = artifact_manifest {
            history.push(artifact_manifest);
        }
        append_skill_context_for_run(workspace_root, prompt, &mut history)
            .map_err(AgentRunPreparationError::Runtime)?;
        return finish_preparation(state, task_id, run_context, history, None, cancellation);
    }

    let prepared_knowledge = prepare_run_knowledge_contexts(
        state,
        task_id,
        &run_context,
        workspace_root,
        config,
        &plan.decision,
        cancellation,
    )
    .map_err(|error| classify_preparation_error(error, cancellation, &run_context))?;
    append_prepared_memory_context(&mut run_context, &mut history, prepared_knowledge.memory);
    if let Some(artifact_manifest) = artifact_manifest {
        history.push(artifact_manifest);
    }
    append_skill_context_for_run(workspace_root, prompt, &mut history)
        .map_err(AgentRunPreparationError::Runtime)?;
    if let Some(workspace_context) = prepared_knowledge.workspace {
        history.push(workspace_context);
    }
    if agent_run_should_stop(cancellation) {
        return Err(AgentRunPreparationError::ControlStop(run_context));
    }
    if cancellation.has_pending_steer() {
        return finish_preparation(state, task_id, run_context, history, None, cancellation);
    }

    cancellation.mark_progress("orchestration", "Preparing execution strategy");
    append_agent_progress_event(state, task_id, &run_context, "Preparing execution strategy")
        .map_err(AgentRunPreparationError::Runtime)?;
    let collaboration_policy = plan.decision.policy();
    append_single_model_policy_guidance(&mut history, &collaboration_policy);
    let collaboration = prepare_agent_collaboration_or_degrade(
        app,
        state,
        config,
        task_id,
        workspace_root,
        &run_context,
        &collaboration_policy,
        prompt,
        &history,
    )
    .map_err(|error| {
        if agent_run_should_stop(cancellation) || error == MODEL_REQUEST_CANCELLED {
            AgentRunPreparationError::ControlStop(run_context.clone())
        } else {
            AgentRunPreparationError::Collaboration {
                error,
                run_context: run_context.clone(),
            }
        }
    })?;
    if let Some(collaboration) = collaboration.as_ref() {
        append_agent_collaboration_context(&mut history, collaboration);
    }
    if agent_run_should_stop(cancellation) {
        return Err(AgentRunPreparationError::ControlStop(run_context));
    }
    finish_preparation(
        state,
        task_id,
        run_context,
        history,
        collaboration,
        cancellation,
    )
}

fn classify_preparation_error(
    error: String,
    cancellation: &Arc<AgentRunControl>,
    run_context: &Metadata,
) -> AgentRunPreparationError {
    if error == MODEL_REQUEST_CANCELLED || agent_run_should_stop(cancellation) {
        AgentRunPreparationError::ControlStop(run_context.clone())
    } else {
        AgentRunPreparationError::Runtime(error)
    }
}

fn finish_preparation(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: Metadata,
    history: Vec<Message>,
    collaboration: Option<AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
) -> Result<PreparedAgentExecution, AgentRunPreparationError> {
    cancellation.mark_progress("executor", "Starting execution");
    append_agent_progress_event(state, task_id, &run_context, "Starting execution")
        .map_err(AgentRunPreparationError::Runtime)?;
    Ok(PreparedAgentExecution {
        run_context,
        history,
        collaboration,
    })
}
