use crate::agent_effort_decision_runtime::record_effort_plan_decision;
use crate::agent_effort_planner::{apply_effort_plan_keys, plan_effort_run};
use crate::agent_failure_terminal_runtime::{
    commit_agent_preparation_failure_terminal, AgentPreparationFailureTerminalOutcome,
};
use crate::agent_model_candidates::effort_model_candidates;
use crate::agent_query_commands::{agent_run_should_stop, append_agent_progress_event};
use crate::agent_run_engine::{
    effort_tier_model, runtime_preparation_error, AgentRunPreparationError, PreparedAgentExecution,
};
use crate::agent_steer_runtime::{apply_pending_agent_steers, AgentSteerApplication};
use crate::app_state::AppState;
use crate::collaboration_service::AgentCollaboration;
use crate::collaboration_stage_runtime::CollaborationStageError;
use crate::configuration_models::ProviderConfig;
use crate::event_persistence::append_event;
use crate::memory_runtime::{
    append_prepared_memory_context, append_skill_context_for_run, commit_prepared_memory_recall,
    prepare_run_knowledge_contexts, MEMORY_RECALL_STALE_ERROR,
};
use crate::project_instructions_runtime::append_project_instructions_context_for_run;
use crate::runtime_values::{add_image_generation_run_context, truncate_for_collaboration};
use crate::session_context_service::prepare_session_history_context;
use agent_core::run_decision_enums::AgentEffectAuthority;
use agent_core::{
    AgentPolicy, AgentRouteRequirements, AgentToolRequirement, EventKind, Message, MessageRole,
    Metadata, OrchestrationPolicy, TaskId,
};
use agent_runtime::{
    prompt_completion_intent, prompt_replaces_prior_objective, AgentLoopState, AgentRunControl,
    PromptEffectAuthority, PromptToolRequirement, RunPreparationCommit,
};
use model_provider::MODEL_REQUEST_CANCELLED;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

enum PreparationFailureAction {
    Finished(Box<Result<crate::view_models::AgentState, String>>),
    RestartAfterSteer,
    ControlStop(Metadata),
}

fn settle_preparation_failure(
    state: &AppState,
    cancellation: &AgentRunControl,
    error: AgentRunPreparationError,
) -> PreparationFailureAction {
    let (message, run_context) = match error {
        AgentRunPreparationError::Finished(result) => {
            return PreparationFailureAction::Finished(result)
        }
        AgentRunPreparationError::ControlStop(run_context) => {
            return PreparationFailureAction::ControlStop(run_context)
        }
        AgentRunPreparationError::Runtime { error, run_context } => (error, run_context),
    };
    match commit_agent_preparation_failure_terminal(state, &run_context, cancellation, message) {
        AgentPreparationFailureTerminalOutcome::Finished(result) => {
            PreparationFailureAction::Finished(Box::new(result))
        }
        AgentPreparationFailureTerminalOutcome::RestartAfterSteer => {
            PreparationFailureAction::RestartAfterSteer
        }
        AgentPreparationFailureTerminalOutcome::Stopped => {
            PreparationFailureAction::ControlStop(run_context)
        }
    }
}

mod memory_evaluation_constraint;
pub(crate) use memory_evaluation_constraint::AgentMemoryEvaluationConstraint;

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
                    | "project_instructions"
                    | "single_model_policy_guidance"
                    | "agent_evidence_packet"
                    | "collaboration_tool_evidence"
                    | "workflow_execution_contract"
                    | "confirmed_plan"
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
        "collaboration_policy",
        "conductor_contract",
        "requested_policy",
        "agent_model",
        "run_decision",
        "execution_plan_semantic_sha256",
        "agent_strategy_receipt_schema",
        "agent_strategy_receipt_status",
        "agent_strategy_receipt_key",
        "agent_strategy_receipt_steer_epoch",
        "agent_strategy_receipt_plan_sha256",
        "memory_ids",
        "memory_selected_count",
        "routed_memory_policy",
        "effective_memory_policy",
        "project_instructions_schema",
        "project_instructions_count",
        "project_instructions_digest",
        "project_instructions_truncated",
        "project_instructions_omitted_json",
        "confirmed_plan_schema",
        "confirmed_plan_digest",
    ] {
        run_context.remove(key);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_execution_replay(
    _host: &dyn crate::desktop_event_sink::AgentRunHost,
    state: &AppState,
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
    let base_run_context = run_context.clone();
    if !cancellation.begin_preparation() {
        return Err(AgentRunPreparationError::ControlStop(run_context));
    }
    macro_rules! settle_failure {
        ($error:expr) => {{
            match settle_preparation_failure(state, cancellation, $error) {
                PreparationFailureAction::Finished(result) => {
                    return Err(AgentRunPreparationError::Finished(result))
                }
                PreparationFailureAction::ControlStop(run_context) => {
                    return Err(AgentRunPreparationError::ControlStop(run_context))
                }
                PreparationFailureAction::RestartAfterSteer => continue,
            }
        }};
    }
    macro_rules! preparation_try {
        ($result:expr) => {{
            match $result {
                Ok(value) => value,
                Err(error) => settle_failure!(error),
            }
        }};
    }
    loop {
        run_context = base_run_context.clone();
        reset_preparation_run_context(&mut run_context);
        if agent_run_should_stop(cancellation) {
            return Err(AgentRunPreparationError::ControlStop(run_context));
        }
        if cancellation.has_pending_steer() {
            prompt = preparation_try!(apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            ));
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
        let (mut base_history, active_user) =
            preparation_try!(preparation_prompt_parts(&runtime.messages)
                .map_err(|error| runtime_preparation_error(&run_context, error)));
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
        let mut history = preparation_try!(prepare_session_history_context(
            state,
            workspace_root,
            &run_context,
            base_history,
            config.context_window_tokens,
            Some(cancellation.as_ref()),
        )
        .map_err(|error| {
            runtime_preparation_error(&run_context, format!("context preparation failed: {error}"))
        }));

        cancellation.mark_progress_at(
            preparation_epoch,
            "strategy",
            "Selecting the execution path",
        );
        let session_model = run_context
            .get("session_agent_model")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let primary_model = match session_model {
            Some(model) => model.to_string(),
            None => effort_tier_model(config, effort.label()),
        };
        let mut effort_plan = plan_effort_run(effort.label(), primary_model, &planning_objective);
        preparation_try!(effort_plan
            .apply_route_requirements(route_requirements)
            .map_err(|error| runtime_preparation_error(&run_context, error)));
        preparation_try!(route_requirements
            .validate_decision(
                &effort_plan.primary_model,
                AgentToolRequirement::from_label(&effort_plan.tool_requirement),
                effort_plan.vision_required,
                &effort_model_candidates(config, effort.label()),
            )
            .map_err(|error| {
                runtime_preparation_error(
                    &run_context,
                    format!("effort-tier model cannot satisfy runtime route requirements: {error}"),
                )
            }));
        preparation_try!(apply_effort_plan_keys(&effort_plan, &mut run_context)
            .map_err(|error| runtime_preparation_error(&run_context, error)));
        if cancellation.has_pending_steer() {
            prompt = preparation_try!(apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            ));
            continue;
        }
        if agent_run_should_stop(cancellation) {
            return Err(AgentRunPreparationError::ControlStop(run_context));
        }
        match record_effort_plan_decision(
            state,
            task_id,
            &mut run_context,
            &effort_plan,
            cancellation,
        ) {
            Ok(()) => {}
            Err(CollaborationStageError::SteerInterrupted) => {
                prompt = preparation_try!(apply_preparation_steer(
                    state,
                    workspace_root,
                    &mut runtime,
                    &run_context,
                    &prompt,
                    cancellation,
                ));
                continue;
            }
            Err(CollaborationStageError::RunStopped) => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            Err(error) => settle_failure!(runtime_preparation_error(&run_context, error.message())),
        }
        let memory_constraint =
            preparation_try!(AgentMemoryEvaluationConstraint::from_context(&run_context)
                .map_err(|error| runtime_preparation_error(&run_context, error)));
        let knowledge_decision = if memory_constraint.is_native() {
            effort_plan.knowledge.clone()
        } else {
            memory_constraint.apply_after_routing(&mut run_context, &effort_plan.knowledge)
        };

        let prepared_knowledge = match prepare_run_knowledge_contexts(
            state,
            task_id,
            &run_context,
            workspace_root,
            config,
            &knowledge_decision,
            cancellation,
            preparation_epoch,
        ) {
            Ok(prepared) => prepared,
            Err(_) if agent_run_should_stop(cancellation) => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            Err(_) if !cancellation.preparation_epoch_is_current(preparation_epoch) => {
                prompt = preparation_try!(apply_preparation_steer(
                    state,
                    workspace_root,
                    &mut runtime,
                    &run_context,
                    &prompt,
                    cancellation,
                ));
                continue;
            }
            Err(error) if error == MODEL_REQUEST_CANCELLED => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            Err(error) => settle_failure!(runtime_preparation_error(&run_context, error)),
        };
        append_prepared_memory_context(
            &mut run_context,
            &mut history,
            prepared_knowledge.memory.as_ref(),
        );
        if let Some(artifact_manifest) = artifact_manifest.clone() {
            history.push(artifact_manifest);
        }
        preparation_try!(append_skill_context_for_run(
            workspace_root,
            &planning_objective,
            &mut history
        )
        .map_err(|error| runtime_preparation_error(&run_context, error)));
        preparation_try!(append_project_instructions_context_for_run(
            workspace_root,
            &crate::persistence_runtime::project_instructions_config_path(),
            &mut run_context,
            &mut history
        )
        .map_err(|error| runtime_preparation_error(&run_context, error)));
        // A user-confirmed plan rides the same protected-context pipeline as
        // project instructions: it is injected here from durable run events on
        // every (re)preparation and carries no scheduling authority.
        if crate::agent_plan_mode_runtime::plan_mode_requested_in_context(&run_context) {
            let events = preparation_try!(match state.store.lock() {
                Ok(store) => crate::agent_read_model::agent_events_for_session(
                    &store,
                    task_id,
                    run_context.get("session_id").map(String::as_str),
                )
                .map_err(|error| runtime_preparation_error(&run_context, error.to_string())),
                Err(error) => Err(runtime_preparation_error(
                    &run_context,
                    format!("store lock poisoned: {error}")
                )),
            });
            crate::agent_plan_mode_runtime::append_confirmed_plan_context(
                &events,
                &mut run_context,
                &mut history,
            );
        }
        if let Some(workspace_context) = prepared_knowledge.workspace {
            history.push(workspace_context);
        }
        if agent_run_should_stop(cancellation) {
            return Err(AgentRunPreparationError::ControlStop(run_context));
        }
        if !cancellation.preparation_epoch_is_current(preparation_epoch) {
            prompt = preparation_try!(apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            ));
            continue;
        }
        if cancellation.has_pending_steer() {
            prompt = preparation_try!(apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            ));
            continue;
        }

        cancellation.mark_progress_at(
            preparation_epoch,
            "orchestration",
            "Preparing execution strategy",
        );
        preparation_try!(append_agent_progress_event(
            state,
            task_id,
            &run_context,
            "Preparing execution strategy",
        )
        .map_err(|error| runtime_preparation_error(&run_context, error)));
        let collaboration_policy = OrchestrationPolicy::Single;
        append_single_model_policy_guidance(&mut history, &collaboration_policy);
        let collaboration: Option<AgentCollaboration> = None;
        if cancellation.has_pending_steer() {
            prompt = preparation_try!(apply_preparation_steer(
                state,
                workspace_root,
                &mut runtime,
                &run_context,
                &prompt,
                cancellation,
            ));
            continue;
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
            Err(error) => settle_failure!(runtime_preparation_error(&run_context, error)),
        };
        match preparation_commit {
            RunPreparationCommit::Committed { .. } => {
                cancellation.mark_progress_at(preparation_epoch, "executor", "Starting execution");
            }
            RunPreparationCommit::Stopped(_) => {
                return Err(AgentRunPreparationError::ControlStop(run_context))
            }
            RunPreparationCommit::RestartAfterSteer => {
                prompt = preparation_try!(apply_preparation_steer(
                    state,
                    workspace_root,
                    &mut runtime,
                    &run_context,
                    &prompt,
                    cancellation,
                ));
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
    state: &AppState,
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

const EFFECTIVE_OBJECTIVE_MAX_CHARS: usize = 6_000;
const EFFECTIVE_OBJECTIVE_INITIAL_FLOOR: usize = 3_000;

fn compact_objective_text(value: &str, max_chars: usize) -> String {
    let length = value.chars().count();
    if length <= max_chars {
        return value.to_string();
    }
    const OMISSION: &str = "\n[...omitted...]\n";
    let omission_chars = OMISSION.chars().count();
    if max_chars <= omission_chars {
        return value.chars().take(max_chars).collect();
    }
    let retained = max_chars - omission_chars;
    let head_chars = retained.saturating_mul(2) / 3;
    let tail_chars = retained - head_chars;
    let tail = value
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!(
        "{}{}{}",
        value.chars().take(head_chars).collect::<String>(),
        OMISSION,
        tail
    )
}

fn fair_turn_budgets(lengths: &[usize], total_budget: usize) -> Vec<usize> {
    let mut budgets = vec![0; lengths.len()];
    let mut pending = (0..lengths.len()).collect::<Vec<_>>();
    let mut remaining = total_budget;
    while !pending.is_empty() && remaining > 0 {
        let share = remaining / pending.len();
        let completed = pending
            .iter()
            .copied()
            .filter(|index| lengths[*index] <= share)
            .collect::<Vec<_>>();
        if completed.is_empty() {
            for (offset, index) in pending.iter().copied().enumerate() {
                budgets[index] = share + usize::from(offset < remaining % pending.len());
            }
            break;
        }
        for index in &completed {
            budgets[*index] = lengths[*index];
            remaining = remaining.saturating_sub(lengths[*index]);
        }
        pending.retain(|index| !completed.contains(index));
    }
    budgets
}

pub(crate) fn cumulative_effective_prompt_objective(
    turns: impl IntoIterator<Item = String>,
) -> Option<String> {
    let mut seen = BTreeSet::new();
    let turns = turns
        .into_iter()
        .map(|turn| turn.trim().to_string())
        .filter(|turn| !turn.is_empty())
        .filter(|turn| seen.insert(turn.clone()))
        .collect::<Vec<_>>();
    if turns.is_empty() {
        return None;
    }
    if turns.len() == 1 {
        return Some(compact_objective_text(
            &turns[0],
            EFFECTIVE_OBJECTIVE_MAX_CHARS,
        ));
    }

    let labels = (0..turns.len())
        .map(|index| {
            if index == 0 {
                "Initial request:\n".to_string()
            } else {
                format!("\n\nAccepted steering {index}:\n")
            }
        })
        .collect::<Vec<_>>();
    let label_chars = labels
        .iter()
        .map(|label| label.chars().count())
        .sum::<usize>();
    let content_budget = EFFECTIVE_OBJECTIVE_MAX_CHARS.saturating_sub(label_chars);
    let steering_lengths = turns[1..]
        .iter()
        .map(|turn| turn.chars().count())
        .collect::<Vec<_>>();
    let initial_floor = content_budget.min(EFFECTIVE_OBJECTIVE_INITIAL_FLOOR);
    let steering_budget = content_budget.saturating_sub(initial_floor);
    let steering_budgets = if steering_lengths.iter().sum::<usize>() <= steering_budget {
        steering_lengths.clone()
    } else {
        fair_turn_budgets(&steering_lengths, steering_budget)
    };
    let initial_budget = content_budget.saturating_sub(steering_budgets.iter().sum::<usize>());
    let mut objective = String::new();
    for (index, turn) in turns.iter().enumerate() {
        objective.push_str(&labels[index]);
        let budget = if index == 0 {
            initial_budget
        } else {
            steering_budgets[index - 1]
        };
        objective.push_str(&compact_objective_text(turn, budget));
    }
    Some(compact_objective_text(
        &objective,
        EFFECTIVE_OBJECTIVE_MAX_CHARS,
    ))
}

pub(crate) fn effective_prompt_objective_for_messages(
    initial_prompt: &str,
    messages: &[Message],
) -> String {
    cumulative_effective_prompt_objective(
        std::iter::once(initial_prompt.to_string()).chain(
            messages
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .filter(|message| {
                    message.metadata.get("queue_mode").map(String::as_str) == Some("steer")
                })
                .map(|message| {
                    message
                        .metadata
                        .get("display_content")
                        .cloned()
                        .unwrap_or_else(|| message.content.clone())
                }),
        ),
    )
    .unwrap_or_else(|| truncate_for_collaboration(initial_prompt, 6_000))
}
