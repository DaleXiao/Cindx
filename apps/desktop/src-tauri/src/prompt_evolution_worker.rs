use crate::app_state::AppState;
use crate::background_work_runtime::wait_for_foreground_agent_idle;
use crate::configuration_models::ProviderConfig;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_models::{
    notify_prompt_evaluation_worker, prompt_evaluation_inflight, wait_for_prompt_evaluation_worker,
};
use crate::prompt_pairwise_runtime::run_background_prompt_pairwise_evaluation;
use crate::run_lifecycle::{ExclusiveKeyLease, RegisteredRunControl};
use crate::runtime_constants::{
    BACKGROUND_WORK_IDLE_GRACE_MS, PROMPT_EVOLUTION_BACKGROUND_BATCH_LIMIT,
    PROMPT_EVOLUTION_BACKGROUND_CAMPAIGN_LIMIT,
};
use crate::runtime_values::unique_id;
use agent_core::{Event, EventKind, Metadata, TaskId};
use agent_runtime::AgentRunControl;
use model_provider::MODEL_REQUEST_CANCELLED;
use orchestrator::ConductorPromptGenome;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

const PROMPT_EVALUATION_REQUEST_SCHEMA: &str = "cindx.prompt-evaluation-request.v1";
const REQUEST_EVENT: &str = "Conductor prompt evaluation requested";
const CHECKPOINT_EVENT: &str = "Conductor prompt evaluation request checkpointed";
const COMPLETED_EVENT: &str = "Conductor prompt evaluation request completed";
const FAILED_EVENT: &str = "Conductor prompt evaluation request failed";
const REQUEST_METADATA_KEY: &str = "prompt_evaluation_request";
const REQUEST_ID_KEY: &str = "prompt_evaluation_request_id";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PromptEvaluationRequest {
    schema: String,
    request_id: String,
    task_id: String,
    run_context: Metadata,
    effort: String,
    policy: String,
    worker_models: Vec<String>,
    agent_budget: usize,
    current_profile: ConductorPromptGenome,
}

impl PromptEvaluationRequest {
    #[allow(clippy::too_many_arguments)]
    fn new(
        task_id: &TaskId,
        run_context: &Metadata,
        effort: String,
        policy: String,
        worker_models: Vec<String>,
        agent_budget: usize,
        current_profile: ConductorPromptGenome,
    ) -> Self {
        Self {
            schema: PROMPT_EVALUATION_REQUEST_SCHEMA.to_string(),
            request_id: unique_id("prompt-evaluation-request"),
            task_id: task_id.0.clone(),
            run_context: persistent_prompt_evaluation_context(run_context),
            effort,
            policy,
            worker_models,
            agent_budget,
            current_profile,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_EVALUATION_REQUEST_SCHEMA {
            return Err("unsupported prompt evaluation request schema".to_string());
        }
        if self.request_id.trim().is_empty()
            || self.task_id.trim().is_empty()
            || !matches!(self.effort.as_str(), "auto" | "pro")
            || self.worker_models.is_empty()
            || !(1..=3).contains(&self.agent_budget)
        {
            return Err("prompt evaluation request is incomplete".to_string());
        }
        self.current_profile.validate()
    }
}

#[derive(Debug, Clone)]
struct PendingPromptEvaluation {
    sequence: u64,
    request: PromptEvaluationRequest,
    completed_actions: usize,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_prompt_pairwise_evaluation(
    app: &tauri::AppHandle,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: String,
    policy: String,
    worker_models: Vec<String>,
    agent_budget: usize,
    current_profile: ConductorPromptGenome,
) -> Result<(), String> {
    let request = PromptEvaluationRequest::new(
        task_id,
        run_context,
        effort,
        policy,
        worker_models,
        agent_budget,
        current_profile,
    );
    request.validate()?;
    let payload = serde_json::to_string(&request)
        .map_err(|error| format!("prompt evaluation request serialization failed: {error}"))?;
    let state = app.state::<AppState>();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        REQUEST_EVENT,
        metadata_with_context(
            [
                (REQUEST_ID_KEY.to_string(), request.request_id.clone()),
                (REQUEST_METADATA_KEY.to_string(), payload),
                ("background_evaluation".to_string(), "true".to_string()),
                ("prompt_effort".to_string(), request.effort.clone()),
            ]
            .into_iter()
            .collect(),
            &request.run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    drop(store);
    notify_prompt_evaluation_worker();
    Ok(())
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
    let mut wake_revision = 0u64;
    loop {
        let state = app.state::<AppState>();
        if state.allow_exit.load(std::sync::atomic::Ordering::Relaxed) {
            break;
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
            if let Err(error) = process_pending_prompt_evaluation(&state, &pending) {
                if error != MODEL_REQUEST_CANCELLED {
                    let _ = finish_prompt_evaluation_request(
                        &state,
                        &pending.request,
                        FAILED_EVENT,
                        pending.completed_actions,
                        "failed",
                        Some(&error),
                    );
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
    let terminal = events
        .iter()
        .filter(|event| matches!(event.summary.as_str(), COMPLETED_EVENT | FAILED_EVENT))
        .filter_map(|event| event.metadata.get(REQUEST_ID_KEY).cloned())
        .collect::<BTreeSet<_>>();
    let checkpoints = events
        .iter()
        .filter(|event| event.summary == CHECKPOINT_EVENT)
        .filter_map(|event| {
            Some((
                event.metadata.get(REQUEST_ID_KEY)?.clone(),
                event
                    .metadata
                    .get("completed_actions")?
                    .parse::<usize>()
                    .ok()?,
            ))
        })
        .fold(
            BTreeMap::<String, usize>::new(),
            |mut values, (id, count)| {
                values
                    .entry(id)
                    .and_modify(|current| *current = (*current).max(count))
                    .or_insert(count);
                values
            },
        );
    let mut latest = BTreeMap::<String, PendingPromptEvaluation>::new();
    for event in events.iter().filter(|event| event.summary == REQUEST_EVENT) {
        let Some(payload) = event.metadata.get(REQUEST_METADATA_KEY) else {
            continue;
        };
        let request = serde_json::from_str::<PromptEvaluationRequest>(payload)
            .map_err(|error| format!("prompt evaluation request is invalid: {error}"))?;
        request.validate()?;
        let candidate = PendingPromptEvaluation {
            sequence: event.sequence,
            completed_actions: checkpoints
                .get(&request.request_id)
                .copied()
                .unwrap_or_default(),
            request,
        };
        let replace = latest
            .get(&candidate.request.effort)
            .is_none_or(|current| candidate.sequence > current.sequence);
        if replace {
            latest.insert(candidate.request.effort.clone(), candidate);
        }
    }
    Ok(latest
        .into_values()
        .filter(|pending| !terminal.contains(&pending.request.request_id))
        .collect())
}

fn process_pending_prompt_evaluation(
    state: &tauri::State<'_, AppState>,
    pending: &PendingPromptEvaluation,
) -> Result<(), String> {
    let request = &pending.request;
    if pending.completed_actions >= PROMPT_EVOLUTION_BACKGROUND_CAMPAIGN_LIMIT {
        return finish_prompt_evaluation_request(
            state,
            request,
            COMPLETED_EVENT,
            pending.completed_actions,
            "campaign_limit",
            None,
        );
    }
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
            pending.completed_actions,
            "disabled_or_unconfigured",
            None,
        );
    }
    validate_request_models(&config, request)?;
    let Some(inflight_lease) = ExclusiveKeyLease::try_acquire(
        prompt_evaluation_inflight(),
        request.effort.clone(),
        "prompt evaluation inflight",
    )?
    else {
        return Ok(());
    };
    let control = Arc::new(AgentRunControl::new("pro"));
    let control_lease = RegisteredRunControl::register(
        &state.prompt_evaluation_controls,
        request.effort.clone(),
        Arc::clone(&control),
        "prompt evaluation control",
        "prompt evaluation is already active for this effort",
    )?;
    let _inflight_lease = inflight_lease;
    let _control_lease = control_lease;
    if !wait_for_foreground_agent_idle(
        state,
        &control,
        Duration::from_millis(BACKGROUND_WORK_IDLE_GRACE_MS),
    )? {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }

    let mut completed_actions = pending.completed_actions;
    let batch_remaining = PROMPT_EVOLUTION_BACKGROUND_CAMPAIGN_LIMIT
        .saturating_sub(completed_actions)
        .min(PROMPT_EVOLUTION_BACKGROUND_BATCH_LIMIT);
    for _ in 0..batch_remaining {
        match run_background_prompt_pairwise_evaluation(
            state,
            &config,
            &TaskId(request.task_id.clone()),
            &request.run_context,
            &request.effort,
            &request.policy,
            &request.worker_models,
            request.agent_budget,
            &request.current_profile,
            &control,
        ) {
            Ok(true) => completed_actions = completed_actions.saturating_add(1),
            Ok(false) => {
                return finish_prompt_evaluation_request(
                    state,
                    request,
                    COMPLETED_EVENT,
                    completed_actions,
                    "converged_or_no_work",
                    None,
                );
            }
            Err(error) if error == MODEL_REQUEST_CANCELLED => {
                checkpoint_prompt_evaluation_request(
                    state,
                    request,
                    completed_actions,
                    "foreground_preempted",
                )?;
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }
    if completed_actions >= PROMPT_EVOLUTION_BACKGROUND_CAMPAIGN_LIMIT {
        finish_prompt_evaluation_request(
            state,
            request,
            COMPLETED_EVENT,
            completed_actions,
            "campaign_limit",
            None,
        )
    } else {
        checkpoint_prompt_evaluation_request(state, request, completed_actions, "batch_complete")
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
    Ok(())
}

fn checkpoint_prompt_evaluation_request(
    state: &tauri::State<'_, AppState>,
    request: &PromptEvaluationRequest,
    completed_actions: usize,
    reason: &str,
) -> Result<(), String> {
    append_request_status(
        state,
        request,
        CHECKPOINT_EVENT,
        completed_actions,
        reason,
        None,
    )
}

fn finish_prompt_evaluation_request(
    state: &tauri::State<'_, AppState>,
    request: &PromptEvaluationRequest,
    summary: &str,
    completed_actions: usize,
    reason: &str,
    error: Option<&str>,
) -> Result<(), String> {
    append_request_status(state, request, summary, completed_actions, reason, error)
}

fn append_request_status(
    state: &tauri::State<'_, AppState>,
    request: &PromptEvaluationRequest,
    summary: &str,
    completed_actions: usize,
    reason: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let mut metadata = [
        (REQUEST_ID_KEY.to_string(), request.request_id.clone()),
        ("background_evaluation".to_string(), "true".to_string()),
        ("prompt_effort".to_string(), request.effort.clone()),
        (
            "completed_actions".to_string(),
            completed_actions.to_string(),
        ),
        ("request_status".to_string(), reason.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(error) = error {
        metadata.insert("error".to_string(), error.chars().take(2_000).collect());
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

fn persistent_prompt_evaluation_context(run_context: &Metadata) -> Metadata {
    const SAFE_KEYS: [&str; 8] = [
        "agent_run_id",
        "agent_effort",
        "collaboration_policy",
        "project_id",
        "project_root",
        "prompt_profile",
        "session_id",
        "task_class",
    ];
    SAFE_KEYS
        .into_iter()
        .filter_map(|key| {
            run_context
                .get(key)
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind};

    fn request(effort: &str, id: &str) -> PromptEvaluationRequest {
        PromptEvaluationRequest {
            schema: PROMPT_EVALUATION_REQUEST_SCHEMA.to_string(),
            request_id: id.to_string(),
            task_id: "task".to_string(),
            run_context: Metadata::new(),
            effort: effort.to_string(),
            policy: "auto".to_string(),
            worker_models: vec!["model".to_string()],
            agent_budget: 1,
            current_profile: ConductorPromptGenome::seed_for_effort(effort),
        }
    }

    fn event(sequence: u64, summary: &str, metadata: Metadata) -> Event {
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: TaskId("task".to_string()),
            sequence,
            timestamp_ms: sequence,
            kind: EventKind::TaskStatusChanged,
            summary: summary.to_string(),
            metadata,
        }
    }

    fn request_event(sequence: u64, request: &PromptEvaluationRequest) -> Event {
        event(
            sequence,
            REQUEST_EVENT,
            [
                (REQUEST_ID_KEY.to_string(), request.request_id.clone()),
                (
                    REQUEST_METADATA_KEY.to_string(),
                    serde_json::to_string(request).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        )
    }

    #[test]
    fn newest_request_supersedes_an_older_request_for_the_same_effort() {
        let older = request("auto", "older");
        let newer = request("auto", "newer");
        let pending = latest_pending_prompt_evaluations_from_events(&[
            request_event(1, &older),
            request_event(2, &newer),
        ])
        .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].request.request_id, "newer");
    }

    #[test]
    fn terminal_latest_request_does_not_resurrect_an_older_request() {
        let older = request("auto", "older");
        let newer = request("auto", "newer");
        let pending = latest_pending_prompt_evaluations_from_events(&[
            request_event(1, &older),
            request_event(2, &newer),
            event(
                3,
                COMPLETED_EVENT,
                [(REQUEST_ID_KEY.to_string(), "newer".to_string())]
                    .into_iter()
                    .collect(),
            ),
        ])
        .unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn checkpoint_preserves_campaign_progress_for_restart() {
        let request = request("pro", "resume");
        let pending = latest_pending_prompt_evaluations_from_events(&[
            request_event(1, &request),
            event(
                2,
                CHECKPOINT_EVENT,
                [
                    (REQUEST_ID_KEY.to_string(), "resume".to_string()),
                    ("completed_actions".to_string(), "4".to_string()),
                ]
                .into_iter()
                .collect(),
            ),
        ])
        .unwrap();
        assert_eq!(pending[0].completed_actions, 4);
    }
}
