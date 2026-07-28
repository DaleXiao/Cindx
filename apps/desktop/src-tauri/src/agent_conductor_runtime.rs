use crate::app_state::AppState;
use crate::collaboration_execution::CollaborationCallLimits;
use crate::collaboration_stage_runtime::{
    run_conductor_collaboration_stage, CollaborationStageError,
};
use crate::configuration_models::{AgentEffort, ProviderConfig};
use agent_core::{Metadata, ModelRole, TaskId};
use orchestrator::{
    AgentRunDecision, AgentRunDecisionHarness, ModelCandidate, CONDUCTOR_MAX_ATTEMPTS,
};
use std::collections::BTreeSet;
use std::time::Duration;

const CONDUCTOR_REPAIR_RECOVERY_WINDOW: Duration = Duration::from_secs(60);
const CONDUCTOR_NO_PROGRESS_TIMEOUT: Duration = Duration::from_secs(45);

pub(crate) fn conductor_repair_recovery_window(has_alternate_model: bool) -> Option<Duration> {
    has_alternate_model.then_some(CONDUCTOR_REPAIR_RECOVERY_WINDOW)
}

pub(crate) fn conductor_call_limits(
    has_alternate_model: bool,
    repair: bool,
) -> CollaborationCallLimits {
    CollaborationCallLimits {
        recovery_window: repair
            .then(|| conductor_repair_recovery_window(has_alternate_model))
            .flatten(),
        no_progress_timeout: has_alternate_model.then_some(CONDUCTOR_NO_PROGRESS_TIMEOUT),
        objective_epoch: None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn attempt_conductor_decision(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    decision_id: &str,
    model_index: usize,
    has_alternate_model: bool,
    conductor_model: &str,
    harness: &AgentRunDecisionHarness,
    attempts: &mut usize,
) -> Result<AgentRunDecision, CollaborationStageError> {
    let stage = format!("run_decision_model_{}", model_index + 1);
    *attempts += 1;
    let response = run_conductor_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        decision_id,
        &stage,
        ModelRole::Planner,
        conductor_model,
        harness.planning_prompt(),
        conductor_call_limits(has_alternate_model, false),
    )?;
    match harness.parse(&response) {
        Ok(decision) => Ok(decision),
        Err(initial_error) if CONDUCTOR_MAX_ATTEMPTS > 1 => {
            *attempts += 1;
            let repair_stage = format!("{stage}_repair");
            let repaired = run_conductor_collaboration_stage(
                state,
                config,
                task_id,
                run_context,
                decision_id,
                &repair_stage,
                ModelRole::Planner,
                conductor_model,
                harness.repair_prompt(&response, &initial_error),
                conductor_call_limits(has_alternate_model, true),
            )?;
            harness.parse(&repaired).map_err(|repair_error| {
                CollaborationStageError::Failed(format!(
                    "initial decision rejected ({initial_error}); repaired decision rejected ({repair_error})"
                ))
            })
        }
        Err(error) => Err(CollaborationStageError::Failed(format!(
            "conductor decision rejected: {error}"
        ))),
    }
}

pub(crate) fn conductor_model_sequence(config: &ProviderConfig) -> Vec<String> {
    let mut seen = BTreeSet::new();
    [
        config.model_for_conductor(),
        config.model_for_role(&ModelRole::Planner),
        config.model_for_role(&ModelRole::Reviewer),
        config.model_for_role(&ModelRole::Summarizer),
        config.model_for_role(&ModelRole::Executor),
        config.model.clone(),
    ]
    .into_iter()
    .filter_map(|model| {
        let model = model.trim();
        (!model.is_empty() && seen.insert(model.to_string())).then(|| model.to_string())
    })
    .collect()
}

pub(crate) fn unique_configured_models(candidates: &[ModelCandidate]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    candidates
        .iter()
        .filter_map(|candidate| {
            let model = candidate.name.trim();
            (!model.is_empty() && seen.insert(model.to_string())).then(|| model.to_string())
        })
        .collect()
}

pub(crate) fn preferred_fallback_model(
    config: &ProviderConfig,
    effort: AgentEffort,
    allowed_models: &[String],
) -> String {
    let preferred = if effort == AgentEffort::Fast && !config.model.trim().is_empty() {
        config.model.trim()
    } else {
        config.executor_model.trim()
    };
    if allowed_models.iter().any(|model| model == preferred) {
        preferred.to_string()
    } else {
        allowed_models
            .first()
            .cloned()
            .unwrap_or_else(|| config.model_for_role(&ModelRole::Executor))
    }
}
