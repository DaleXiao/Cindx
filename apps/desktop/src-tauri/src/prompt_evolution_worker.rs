use crate::app_state::AppState;
use crate::background_work_runtime::wait_for_foreground_agent_idle;
use crate::configuration_models::ProviderConfig;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_campaign_budget::{
    recover_prompt_evaluation_checkpoints, PromptEvaluationCampaignUsage,
};
use crate::prompt_evolution_campaign_runtime::append_prompt_evaluation_action;
use crate::prompt_evolution_models::{
    prompt_evaluation_inflight, wait_for_prompt_evaluation_worker,
};
use crate::prompt_learning_runtime::{
    prompt_evaluation_error_text,
    validate_prompt_evaluation_request_configuration as validate_request_configuration,
};
use crate::prompt_pairwise_runtime::run_background_prompt_pairwise_evaluation;
use crate::runtime_constants::{
    BACKGROUND_WORK_IDLE_GRACE_MS, PROMPT_EVOLUTION_BACKGROUND_BATCH_LIMIT,
    PROMPT_EVOLUTION_BACKGROUND_CAMPAIGN_LIMIT,
};
use crate::runtime_values::current_time_millis;
use agent_core::{Event, EventKind, Metadata, TaskId};
use agent_runtime::AgentRunControl;
use model_provider::MODEL_REQUEST_CANCELLED;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

pub(crate) use crate::prompt_distillation_worker_request::{
    enqueue_prompt_pairwise_evaluation, enqueue_prompt_pro_to_auto_distillation,
    PromptEvaluationRequest, REQUEST_EVENT, REQUEST_ID_KEY, REQUEST_METADATA_KEY,
};
#[cfg(test)]
use crate::prompt_distillation_worker_request::PROMPT_EVALUATION_REQUEST_SCHEMA;

const CHECKPOINT_EVENT: &str = "Conductor prompt evaluation request checkpointed";
const COMPLETED_EVENT: &str = "Conductor prompt evaluation request completed";
const FAILED_EVENT: &str = "Conductor prompt evaluation request failed";

#[derive(Debug, Clone)]
struct PendingPromptEvaluation {
    sequence: u64,
    request: PromptEvaluationRequest,
    completed_actions: usize,
    campaign_usage: PromptEvaluationCampaignUsage,
}

pub(crate) fn start_prompt_evolution_worker(app: tauri::AppHandle) {
    let spawn = std::thread::Builder::new()
        .name("cindx-prompt-evolution".to_string())
        .spawn(move || prompt_evolution_worker_loop(app));
    if let Err(error) = spawn {
        eprintln!("prompt evolution worker could not start: {error}");
    }
}

fn prompt_evolution_worker_loop(app: tauri::AppHandle) {
    if !crate::prompt_attempt_runtime::recover_prompt_evaluation_attempts_at_worker_start(&app) {
        return;
    }
    let mut wake_revision = 0u64;
    loop {
        let state = app.state::<AppState>();
        if state.allow_exit.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if let Err(error) =
            crate::prompt_evolution_transfer_outbox::dispatch_prompt_auto_transfer_intents(&app)
        {
            eprintln!("prompt Auto transfer intent scan failed: {error}");
        }
        if crate::prompt_distillation_outbox::prompt_distillation_outbox_is_idle(&state) {
            if let Err(error) =
                crate::prompt_distillation_outbox::dispatch_prompt_pro_distillation_intents(&app)
            {
                eprintln!("prompt Pro distillation intent scan failed: {error}");
            }
        }
        let pending = match latest_pending_prompt_evaluations(&state) {
            Ok(pending) => pending,
            Err(error) => {
                eprintln!("prompt evolution request scan failed: {error}");
                wait_for_prompt_evaluation_worker(&mut wake_revision, Duration::from_secs(30));
                continue;
            }
        };
        if pending.is_empty() {
            wait_for_prompt_evaluation_worker(&mut wake_revision, Duration::from_secs(60));
            continue;
        }
        for pending in pending {
            if state.allow_exit.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let mut completed_actions = pending.completed_actions;
            let mut campaign_usage = pending.campaign_usage.clone();
            if let Err(error) = process_pending_prompt_evaluation(
                &state,
                &pending,
                &mut completed_actions,
                &mut campaign_usage,
            ) {
                if error != MODEL_REQUEST_CANCELLED {
                    if let Err(status_error) = finish_prompt_evaluation_request(
                        &state,
                        &pending.request,
                        FAILED_EVENT,
                        completed_actions,
                        &campaign_usage,
                        "failed",
                        Some(&error),
                    ) {
                        eprintln!(
                            "prompt evolution failed status could not be persisted: {status_error}"
                        );
                        return;
                    }
                }
            }
        }
    }
}

fn latest_pending_prompt_evaluations(
    state: &tauri::State<'_, AppState>,
) -> Result<Vec<PendingPromptEvaluation>, String> {
    let events = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?
        .list_by_task_and_metadata(
            &crate::runtime_values::phase16_task_id(),
            "background_evaluation",
            "true",
        )
        .map_err(|error| error.to_string())?;
    latest_pending_prompt_evaluations_from_events(&events)
}

fn latest_pending_prompt_evaluations_from_events(
    events: &[Event],
) -> Result<Vec<PendingPromptEvaluation>, String> {
    let mut terminal = BTreeSet::new();
    for event in events
        .iter()
        .filter(|event| matches!(event.summary.as_str(), COMPLETED_EVENT | FAILED_EVENT))
    {
        let request_id = event
            .metadata
            .get(REQUEST_ID_KEY)
            .filter(|request_id| !request_id.trim().is_empty())
            .cloned()
            .ok_or_else(|| "prompt evaluation terminal request id is invalid".to_string())?;
        terminal.insert(request_id);
    }
    let checkpoints = recover_prompt_evaluation_checkpoints(events)?;
    let mut latest = BTreeMap::<(String, String, String), PendingPromptEvaluation>::new();
    let mut request_ids = BTreeSet::new();
    for event in events.iter().filter(|event| event.summary == REQUEST_EVENT) {
        let Some(payload) = event.metadata.get(REQUEST_METADATA_KEY) else {
            continue;
        };
        let request = serde_json::from_str::<PromptEvaluationRequest>(payload)
            .map_err(|error| format!("prompt evaluation request is invalid: {error}"))?;
        request.validate()?;
        request_ids.insert(request.request_id.clone());
        let checkpoint = checkpoints.get(&request.request_id);
        let checkpoint_is_valid = checkpoint.is_none_or(|checkpoint| {
            checkpoint.is_valid_for_request_started_at(event.timestamp_ms)
        });
        let candidate = PendingPromptEvaluation {
            sequence: event.sequence,
            completed_actions: checkpoint
                .map(|checkpoint| checkpoint.completed_actions)
                .unwrap_or_default(),
            campaign_usage: if checkpoint_is_valid {
                checkpoint
                    .map(|checkpoint| checkpoint.campaign_usage.clone())
                    .unwrap_or_else(|| {
                        PromptEvaluationCampaignUsage::starting_at(event.timestamp_ms)
                    })
            } else {
                PromptEvaluationCampaignUsage::fail_closed()
            },
            request,
        };
        let key = candidate.request.scope_key();
        let key = (key.0.to_string(), key.1.to_string(), key.2.to_string());
        let replace = latest
            .get(&key)
            .is_none_or(|current| candidate.sequence > current.sequence);
        if replace {
            latest.insert(key, candidate);
        }
    }
    if checkpoints
        .keys()
        .chain(terminal.iter())
        .any(|request_id| !request_ids.contains(request_id))
    {
        return Err("prompt evaluation checkpoint request id is unknown".to_string());
    }
    Ok(latest
        .into_values()
        .filter(|pending| !terminal.contains(&pending.request.request_id))
        .collect())
}

fn process_pending_prompt_evaluation(
    state: &tauri::State<'_, AppState>,
    pending: &PendingPromptEvaluation,
    completed_actions_out: &mut usize,
    campaign_usage_out: &mut PromptEvaluationCampaignUsage,
) -> Result<(), String> {
    let request = &pending.request;
    if *completed_actions_out >= PROMPT_EVOLUTION_BACKGROUND_CAMPAIGN_LIMIT {
        return finish_prompt_evaluation_request(
            state,
            request,
            COMPLETED_EVENT,
            *completed_actions_out,
            campaign_usage_out,
            "campaign_limit",
            None,
        );
    }
    let Some(campaign_budget) = campaign_usage_out.remaining_budget(current_time_millis()) else {
        return finish_prompt_evaluation_request(
            state,
            request,
            COMPLETED_EVENT,
            *completed_actions_out,
            campaign_usage_out,
            "campaign_resource_budget",
            None,
        );
    };
    let config = state
        .provider_config
        .lock()
        .map_err(|error| format!("provider config lock poisoned: {error}"))?
        .clone();
    if !config.prompt_evolution_enabled || !config.is_ready() {
        return finish_prompt_evaluation_request(
            state,
            request,
            COMPLETED_EVENT,
            *completed_actions_out,
            campaign_usage_out,
            "disabled_or_unconfigured",
            None,
        );
    }
    validate_request_models(&config, request)?;
    let Some(inflight_lease) = prompt_evaluation_inflight()
        .try_acquire(request.effort.clone())
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let control = Arc::new(AgentRunControl::with_budget(campaign_budget));
    let control_lease = state
        .prompt_evaluation_controls
        .register(request.effort.clone(), Arc::clone(&control))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "prompt evaluation is already active for this effort".to_string())?;
    let _inflight_lease = inflight_lease;
    let _control_lease = control_lease;
    if !wait_for_foreground_agent_idle(
        state,
        &control,
        Duration::from_millis(BACKGROUND_WORK_IDLE_GRACE_MS),
    )? {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }

    let mut completed_actions = *completed_actions_out;
    let batch_remaining = PROMPT_EVOLUTION_BACKGROUND_CAMPAIGN_LIMIT
        .saturating_sub(completed_actions)
        .min(PROMPT_EVOLUTION_BACKGROUND_BATCH_LIMIT);
    for _ in 0..batch_remaining {
        let action_index = completed_actions;
        let action_id = format!("{}:{action_index}", request.request_id);
        append_prompt_evaluation_action(
            state,
            &TaskId(request.task_id.clone()),
            &request.run_context,
            &request.request_id,
            &request.effort,
            &action_id,
            action_index,
            completed_actions,
            None,
        )?;
        let result = run_background_prompt_pairwise_evaluation(
            state,
            &config,
            &TaskId(request.task_id.clone()),
            &request.run_context,
            &request.effort,
            &request.policy,
            &request.worker_models,
            request.agent_budget,
            &request.current_profile,
            request.pro_teacher_snapshot.as_ref(),
            &control,
        );
        *campaign_usage_out = pending.campaign_usage.with_control_usage(&control);
        let next_completed_actions =
            completed_actions.saturating_add(usize::from(matches!(&result, Ok(true))));
        append_prompt_evaluation_action(
            state,
            &TaskId(request.task_id.clone()),
            &request.run_context,
            &request.request_id,
            &request.effort,
            &action_id,
            action_index,
            next_completed_actions,
            Some(campaign_usage_out),
        )?;
        match result {
            Ok(true) => {
                completed_actions = next_completed_actions;
                *completed_actions_out = completed_actions;
            }
            Ok(false) => {
                return finish_prompt_evaluation_request(
                    state,
                    request,
                    COMPLETED_EVENT,
                    completed_actions,
                    campaign_usage_out,
                    "converged_or_no_work",
                    None,
                );
            }
            Err(error) if error == MODEL_REQUEST_CANCELLED => {
                let result = checkpoint_prompt_evaluation_request(
                    state,
                    request,
                    completed_actions,
                    campaign_usage_out,
                    "foreground_preempted",
                );
                return match result {
                    Ok(()) => Err(error),
                    Err(status_error) => Err(status_error),
                };
            }
            Err(error) => {
                return finish_prompt_evaluation_request(
                    state,
                    request,
                    FAILED_EVENT,
                    completed_actions,
                    campaign_usage_out,
                    "evaluation_failed",
                    Some(&error),
                );
            }
        }
    }
    *campaign_usage_out = pending.campaign_usage.with_control_usage(&control);
    if completed_actions >= PROMPT_EVOLUTION_BACKGROUND_CAMPAIGN_LIMIT {
        finish_prompt_evaluation_request(
            state,
            request,
            COMPLETED_EVENT,
            completed_actions,
            campaign_usage_out,
            "campaign_limit",
            None,
        )
    } else if campaign_usage_out
        .remaining_budget(current_time_millis())
        .is_none()
    {
        finish_prompt_evaluation_request(
            state,
            request,
            COMPLETED_EVENT,
            completed_actions,
            campaign_usage_out,
            "campaign_resource_budget",
            None,
        )
    } else {
        checkpoint_prompt_evaluation_request(
            state,
            request,
            completed_actions,
            campaign_usage_out,
            "batch_complete",
        )
    }
}

fn validate_request_models(
    config: &ProviderConfig,
    request: &PromptEvaluationRequest,
) -> Result<(), String> {
    let configured = [
        config.model.as_str(),
        config.conductor_model.as_str(),
        config.planner_model.as_str(),
        config.executor_model.as_str(),
        config.reviewer_model.as_str(),
        config.summarizer_model.as_str(),
    ]
    .into_iter()
    .filter(|model| !model.trim().is_empty())
    .collect::<BTreeSet<_>>();
    if request
        .worker_models
        .iter()
        .any(|model| !configured.contains(model.as_str()))
    {
        return Err(
            "prompt evaluation request model pool no longer matches provider configuration"
                .to_string(),
        );
    }
    let r = request;
    let fingerprint = (
        &r.effort,
        &r.policy,
        &r.worker_models,
        r.agent_budget,
        &r.current_profile,
    );
    validate_request_configuration(config, &r.configuration_sha256, fingerprint)?;
    Ok(())
}

fn finish_prompt_evaluation_request(
    state: &tauri::State<'_, AppState>,
    request: &PromptEvaluationRequest,
    summary: &str,
    completed_actions: usize,
    campaign_usage: &PromptEvaluationCampaignUsage,
    reason: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let encoded_campaign_usage = serde_json::to_string(campaign_usage)
        .map_err(|error| format!("prompt campaign usage serialization failed: {error}"))?;
    let mut metadata = [
        (REQUEST_ID_KEY.to_string(), request.request_id.clone()),
        ("background_evaluation".to_string(), "true".to_string()),
        ("prompt_effort".to_string(), request.effort.clone()),
        (
            "completed_actions".to_string(),
            completed_actions.to_string(),
        ),
        ("campaign_usage".to_string(), encoded_campaign_usage),
        ("request_status".to_string(), reason.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(error) = error {
        metadata.insert("error".to_string(), prompt_evaluation_error_text(error));
    }
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &TaskId(request.task_id.clone()),
        EventKind::TaskStatusChanged,
        summary,
        metadata_with_context(metadata, &request.run_context),
    )
    .map_err(|error| error.to_string())
}

fn checkpoint_prompt_evaluation_request(
    state: &tauri::State<'_, AppState>,
    request: &PromptEvaluationRequest,
    completed_actions: usize,
    campaign_usage: &PromptEvaluationCampaignUsage,
    reason: &str,
) -> Result<(), String> {
    finish_prompt_evaluation_request(
        state,
        request,
        CHECKPOINT_EVENT,
        completed_actions,
        campaign_usage,
        reason,
        None,
    )
}

#[cfg(test)]
#[path = "prompt_evolution_worker_tests.rs"]
mod tests;
