use crate::agent_collaboration_runtime::{
    append_agent_collaboration_context, prepare_agent_collaboration_or_degrade,
};
use crate::agent_query_commands::{agent_run_should_stop, append_agent_progress_event};
use crate::agent_run_engine::{
    runtime_preparation_error, AgentRunPreparationError, PreparedAgentExecution,
};
use crate::agent_steer_runtime::{apply_pending_agent_steers, AgentSteerApplication};
use crate::agent_strategy_runtime::{
    effective_prompt_objective_for_messages, plan_agent_run, AgentRunPlanningRequest,
};
use crate::app_state::AppState;
use crate::collaboration_service::truncate_for_collaboration;
use crate::collaboration_stage_runtime::CollaborationStageError;
use crate::configuration_models::ProviderConfig;
use crate::event_persistence::append_event;
use crate::memory_runtime::{
    append_prepared_memory_context, append_skill_context_for_run, commit_prepared_memory_recall,
    prepare_run_knowledge_contexts, MEMORY_RECALL_STALE_ERROR,
};
use crate::prompt_profile_serving::copy_prompt_profile_assignment;
use crate::runtime_values::add_image_generation_run_context;
use crate::session_context_service::prepare_session_history_context;
use agent_core::{EventKind, Message, MessageRole, Metadata, TaskId};
use agent_runtime::{
    prompt_completion_intent, prompt_replaces_prior_objective, AgentLoopState, AgentRunControl,
    PromptEffectAuthority, PromptToolRequirement, RunPreparationCommit,
};
use model_provider::MODEL_REQUEST_CANCELLED;
use orchestrator::{
    AgentEffectAuthority, AgentPolicy, AgentRouteRequirements, AgentToolRequirement,
    OrchestrationPolicy,
};
use std::path::Path;
use std::sync::Arc;

mod memory_evaluation_constraint;
pub(crate) use memory_evaluation_constraint::AgentMemoryEvaluationConstraint;
#[cfg(feature = "realworld-eval")]
pub(crate) use memory_evaluation_constraint::{
    AGENT_MEMORY_EVALUATION_CONSTRAINT_KEY, EFFECTIVE_MEMORY_POLICY_KEY, ROUTED_MEMORY_POLICY_KEY,
};

fn append_single_model_policy_guidance(history: &mut Vec<Message>, policy: &OrchestrationPolicy) {
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

pub(crate) fn preparation_prompt_parts(
    messages: &[Message],
) -> Result<(Vec<Message>, Message), &'static str> {
    let Some((active_user, history)) = messages.split_last() else {
        return Err("agent runtime has no active user prompt");
    };
    if active_user.role != MessageRole::User {
        return Err("agent runtime does not end with an active user prompt");
    }
    Ok((history.to_vec(), active_user.clone()))
}

pub(crate) fn route_requirements_for_preparation(
    run_context: &Metadata,
    messages: &[Message],
    active_user: &Message,
) -> AgentRouteRequirements {
    let completion_intent = prompt_completion_intent(run_context);
    let mut minimum_tool_requirement = match completion_intent.tool_requirement {
        PromptToolRequirement::None => AgentToolRequirement::None,
        PromptToolRequirement::ReadOnly => AgentToolRequirement::ReadOnly,
        PromptToolRequirement::Effects => AgentToolRequirement::Effects,
    };
    let image_generation_required = run_context
        .get("image_generation_required")
        .map(String::as_str)
        == Some("true");
    if image_generation_required {
        minimum_tool_requirement = AgentToolRequirement::Effects;
    }
    let effect_authority = if image_generation_required {
        AgentEffectAuthority::Required
    } else {
        match completion_intent.effect_authority {
            PromptEffectAuthority::Forbidden => AgentEffectAuthority::Forbidden,
            PromptEffectAuthority::Allowed => AgentEffectAuthority::Allowed,
            PromptEffectAuthority::Required => AgentEffectAuthority::Required,
        }
    };
    let message_has_images = |message: &Message| {
        message
            .metadata
            .get("image_paths")
            .is_some_and(|paths| paths.lines().any(|path| !path.trim().is_empty()))
    };
    let active_user_has_images = message_has_images(active_user);
    let image_input_required = if prompt_replaces_prior_objective(run_context) {
        active_user_has_images
    } else {
        let run_ids = [
            run_context.get("agent_run_id").map(String::as_str),
            run_context.get("source_agent_run_id").map(String::as_str),
        ];
        active_user_has_images
            || messages.iter().any(|message| {
                let message_run_id = message.metadata.get("agent_run_id").map(String::as_str);
                message.role == MessageRole::User
                    && message_run_id.is_some()
                    && run_ids.contains(&message_run_id)
                    && message_has_images(message)
            })
    };
    AgentRouteRequirements {
        minimum_tool_requirement,
        effect_authority,
        image_input_required,
    }
}

pub(crate) fn image_generation_objective_for_preparation<'a>(
    run_context: &Metadata,
    effective_objective: &'a str,
    latest_objective: &'a str,
) -> &'a str {
    if prompt_replaces_prior_objective(run_context) {
        latest_objective
    } else {
        effective_objective
    }
}

pub(crate) fn remove_stale_preparation_context(history: &mut Vec<Message>) {
    history.retain(|message| {
        if message.metadata.get("internal").map(String::as_str) != Some("true") {
            return true;
        }
        let kind = message.metadata.get("kind").map(String::as_str);
        let preparation_kind = matches!(
            kind,
            Some(
                "project_memory"
                    | "skill_context"
                    | "knowledge_context"
                    | "single_model_policy_guidance"
                    | "agent_evidence_packet"
                    | "collaboration_tool_evidence"
                    | "workflow_execution_contract"
            )
        );
        !preparation_kind && !message.metadata.contains_key("collaboration_stage")
    });
}

pub(crate) fn reset_preparation_run_context(run_context: &mut Metadata) {
    for key in [
        "prompt_objective",
        "effective_prompt_objective",
        "task_class",
        "tool_requirement",
        "vision_required",
        "route_minimum_tool_requirement",
        "route_image_input_required",
        "route_effect_authority",
        "pre_decision_context_fingerprint",
        "pre_decision_task_class",
        "route_requirements_fingerprint",
        "causal_route_policy",
        "route_prompt_profile_sha256",
        "causal_route_candidate",
        "causal_route_selected",
        "causal_route_selected_action_id",
        "causal_route_reason",
        "causal_route_receipt_sha256",
        "decision_calibration",
        "decision_calibration_reason",
        "candidate_route_tier",
        "selected_route_tier",
        "computation_verdict",
        "computation_benefit_bps",
        "computation_required_value_bps",
        "computation_net_value_bps",
        "computation_units",
        "computation_matched_examples",
        "routing_signature",
        "collaboration_policy",
        "collaboration_profile",
        "conductor_contract",
        "expected_collaboration_uplift_bps",
        "agent_model",
        "router_model",
        "router_examples",
        "router_source",
        "conductor_degraded",
        "conductor_failure",
        "run_decision",
        "run_decision_attempts",
        "conductor_models_attempted",
        "conductor_selected_model",
        "conductor_configured_models",
        "conductor_health_routing",
        "conductor_health_candidate_models",
        "conductor_health_observations",
        "conductor_health_attempts",
        "conductor_hedge_enabled",
        "memory_ids",
        "memory_selected_count",
        "routed_memory_policy",
        "effective_memory_policy",
    ] {
        run_context.remove(key);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_execution_replay(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    mut run_context: Metadata,
    mut runtime: AgentLoopState,
    mut prompt: String,
    artifact_manifest: Option<Message>,
    effort: AgentPolicy,
    cancellation: &Arc<AgentRunControl>,
) -> Result<PreparedAgentExecution, AgentRunPreparationError> {
    let mut base_run_context = run_context.clone();
    if !cancellation.begin_preparation() {
        return Err(AgentRunPreparationError::ControlStop(run_context));
    }
    loop {
        run_context = base_run_context.clone();
        reset_preparation_run_context(&mut run_context);
        if agent_run_should_stop(cancellation) {
            return Err(AgentRunPreparationError::ControlStop(run_context));
        }
        if cancellation.has_pending_steer() {
            prompt = apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            )?;
            continue;
        }
        let preparation_epoch = cancellation.steer_epoch();
        run_context.insert("steer_epoch".to_string(), preparation_epoch.to_string());
        // A no-op control steer may advance `steer_epoch` without changing the
        // active objective. Keep prompt-scoped contracts on the epoch that was
        // actually prepared so permission/recovery does not discard valid work.
        run_context.insert(
            "prompt_contract_epoch".to_string(),
            preparation_epoch.to_string(),
        );
        let initial_prompt = run_context
            .get("initial_prompt_objective")
            .cloned()
            .unwrap_or_else(|| prompt.clone());
        let planning_objective =
            effective_prompt_objective_for_messages(&initial_prompt, &runtime.messages);
        run_context.insert(
            "effective_prompt_objective".to_string(),
            planning_objective.clone(),
        );

        if let (Some(run_id), Some(active_user)) =
            (run_context.get("agent_run_id"), runtime.messages.last_mut())
        {
            if active_user.role == MessageRole::User {
                active_user
                    .metadata
                    .insert("agent_run_id".to_string(), run_id.clone());
            }
        }
        let (mut base_history, active_user) = preparation_prompt_parts(&runtime.messages)
            .map_err(|error| runtime_preparation_error(&run_context, error))?;
        let latest_prompt_objective = active_user
            .metadata
            .get("display_content")
            .map(String::as_str)
            .unwrap_or(&active_user.content);
        run_context.insert(
            "prompt_objective".to_string(),
            truncate_for_collaboration(latest_prompt_objective, 6_000),
        );
        let image_generation_objective = image_generation_objective_for_preparation(
            &run_context,
            &planning_objective,
            latest_prompt_objective,
        );
        add_image_generation_run_context(&mut run_context, config, image_generation_objective);
        let route_requirements =
            route_requirements_for_preparation(&run_context, &runtime.messages, &active_user);
        remove_stale_preparation_context(&mut base_history);
        let mut history = prepare_session_history_context(
            state,
            workspace_root,
            &run_context,
            base_history,
            config.context_window_tokens,
        )
        .map_err(|error| {
            runtime_preparation_error(&run_context, format!("context preparation failed: {error}"))
        })?;

        cancellation.mark_progress_at(
            preparation_epoch,
            "strategy",
            "Selecting the execution path",
        );
        let plan = plan_agent_run(
            state,
            &mut run_context,
            AgentRunPlanningRequest {
                config,
                task_id,
                prompt: &planning_objective,
                history: &history,
                effort,
                route_requirements,
                cancellation,
            },
        );
        copy_prompt_profile_assignment(&run_context, &mut base_run_context)
            .map_err(|error| runtime_preparation_error(&run_context, error))?;
        let plan = match plan {
            Ok(plan) => plan,
            Err(CollaborationStageError::SteerInterrupted) => {
                prompt = apply_preparation_steer(
                    state,
                    workspace_root,
                    &mut runtime,
                    &run_context,
                    &prompt,
                    cancellation,
                )?;
                continue;
            }
            Err(CollaborationStageError::RunStopped) => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            Err(error) => return Err(runtime_preparation_error(&run_context, error.message())),
        };
        let memory_constraint = AgentMemoryEvaluationConstraint::from_context(&run_context)
            .map_err(|error| runtime_preparation_error(&run_context, error))?;
        let effective_knowledge_decision = (!memory_constraint.is_native())
            .then(|| memory_constraint.apply_after_routing(&mut run_context, &plan.decision));
        let knowledge_decision = effective_knowledge_decision
            .as_ref()
            .unwrap_or(&plan.decision);

        let prepared_knowledge = match prepare_run_knowledge_contexts(
            state,
            task_id,
            &run_context,
            workspace_root,
            config,
            knowledge_decision,
            cancellation,
            preparation_epoch,
        ) {
            Ok(prepared) => prepared,
            Err(_) if agent_run_should_stop(cancellation) => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            Err(_) if !cancellation.preparation_epoch_is_current(preparation_epoch) => {
                prompt = apply_preparation_steer(
                    state,
                    workspace_root,
                    &mut runtime,
                    &run_context,
                    &prompt,
                    cancellation,
                )?;
                continue;
            }
            Err(error) if error == MODEL_REQUEST_CANCELLED => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            Err(error) => return Err(runtime_preparation_error(&run_context, error)),
        };
        append_prepared_memory_context(
            &mut run_context,
            &mut history,
            prepared_knowledge.memory.as_ref(),
        );
        if let Some(artifact_manifest) = artifact_manifest.clone() {
            history.push(artifact_manifest);
        }
        append_skill_context_for_run(workspace_root, &planning_objective, &mut history)
            .map_err(|error| runtime_preparation_error(&run_context, error))?;
        if let Some(workspace_context) = prepared_knowledge.workspace {
            history.push(workspace_context);
        }
        if agent_run_should_stop(cancellation) {
            return Err(AgentRunPreparationError::ControlStop(run_context));
        }
        if !cancellation.preparation_epoch_is_current(preparation_epoch) {
            prompt = apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            )?;
            continue;
        }
        if cancellation.has_pending_steer() {
            prompt = apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            )?;
            continue;
        }

        cancellation.mark_progress_at(
            preparation_epoch,
            "orchestration",
            "Preparing execution strategy",
        );
        append_agent_progress_event(state, task_id, &run_context, "Preparing execution strategy")
            .map_err(|error| runtime_preparation_error(&run_context, error))?;
        let collaboration_policy = plan.decision.policy();
        append_single_model_policy_guidance(&mut history, &collaboration_policy);
        let collaboration = match prepare_agent_collaboration_or_degrade(
            app,
            state,
            config,
            task_id,
            workspace_root,
            &run_context,
            &collaboration_policy,
            &planning_objective,
            &history,
        ) {
            Ok(collaboration) => collaboration,
            Err(_) if agent_run_should_stop(cancellation) => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            Err(_) if !cancellation.preparation_epoch_is_current(preparation_epoch) => None,
            Err(error) if error == MODEL_REQUEST_CANCELLED => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            Err(error) => {
                return Err(AgentRunPreparationError::Collaboration { error, run_context })
            }
        };
        if cancellation.has_pending_steer() {
            prompt = apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            )?;
            continue;
        }
        if let Some(collaboration) = collaboration.as_ref() {
            append_agent_collaboration_context(&mut history, collaboration);
        }
        if agent_run_should_stop(cancellation) {
            return Err(AgentRunPreparationError::ControlStop(run_context));
        }
        let preparation_commit = cancellation.commit_preparation_with(preparation_epoch, || {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            store
                .with_immediate_transaction(|store| {
                    append_event(
                        store,
                        task_id,
                        EventKind::TaskStatusChanged,
                        "Starting execution",
                        run_context.clone(),
                    )?;
                    commit_prepared_memory_recall(
                        store,
                        task_id,
                        &run_context,
                        prepared_knowledge.memory.as_ref(),
                    )
                })
                .map_err(|error| {
                    if error.message == MEMORY_RECALL_STALE_ERROR {
                        MEMORY_RECALL_STALE_ERROR.to_string()
                    } else {
                        format!("failed to commit agent preparation transaction: {error}")
                    }
                })?;
            history.push(active_user);
            runtime.messages = history;
            Ok::<(), String>(())
        });
        let preparation_commit = match preparation_commit {
            Ok(commit) => commit,
            Err(error) if error == MEMORY_RECALL_STALE_ERROR => continue,
            Err(error) => return Err(runtime_preparation_error(&run_context, error)),
        };
        match preparation_commit {
            RunPreparationCommit::Committed { .. } => {
                cancellation.mark_progress_at(preparation_epoch, "executor", "Starting execution");
            }
            RunPreparationCommit::Stopped(_) => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            RunPreparationCommit::RestartAfterSteer => {
                prompt = apply_preparation_steer(
                    state,
                    workspace_root,
                    &mut runtime,
                    &run_context,
                    &prompt,
                    cancellation,
                )?;
                continue;
            }
        }
        return Ok(PreparedAgentExecution {
            base_run_context: base_run_context.clone(),
            run_context,
            runtime,
            prompt,
            collaboration,
        });
    }
}

fn apply_preparation_steer(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut AgentLoopState,
    run_context: &Metadata,
    current_prompt: &str,
    cancellation: &Arc<AgentRunControl>,
) -> Result<String, AgentRunPreparationError> {
    match apply_pending_agent_steers(state, workspace_root, runtime, run_context, cancellation)
        .map_err(|error| runtime_preparation_error(run_context, error))?
    {
        AgentSteerApplication::Applied(steer) => Ok(steer.prompt),
        AgentSteerApplication::NoPending | AgentSteerApplication::ResolvedNoop { .. } => {
            Ok(current_prompt.to_string())
        }
        AgentSteerApplication::Stopped(_) => {
            Err(AgentRunPreparationError::ControlStop(run_context.clone()))
        }
    }
}
