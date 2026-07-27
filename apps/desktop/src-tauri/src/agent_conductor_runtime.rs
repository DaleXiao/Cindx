use crate::app_state::AppState;
use crate::collaboration_stage_runtime::run_collaboration_stage;
use crate::configuration_models::{AgentEffort, ProviderConfig};
use agent_core::{Metadata, ModelRole, TaskId};
use orchestrator::{
    AgentRunDecision, AgentRunDecisionHarness, ModelCandidate, CONDUCTOR_MAX_ATTEMPTS,
};
use std::collections::BTreeSet;

#[allow(clippy::too_many_arguments)]
pub(crate) fn attempt_conductor_decision(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    decision_id: &str,
    model_index: usize,
    conductor_model: &str,
    harness: &AgentRunDecisionHarness,
    attempts: &mut usize,
) -> Result<AgentRunDecision, String> {
    let stage = format!("run_decision_model_{}", model_index + 1);
    *attempts += 1;
    let response = run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        decision_id,
        &stage,
        ModelRole::Planner,
        conductor_model,
        harness.planning_prompt(),
    )?;
    match harness.parse(&response) {
        Ok(decision) => Ok(decision),
        Err(initial_error) if CONDUCTOR_MAX_ATTEMPTS > 1 => {
            *attempts += 1;
            let repair_stage = format!("{stage}_repair");
            let repaired = run_collaboration_stage(
                state,
                config,
                task_id,
                run_context,
                decision_id,
                &repair_stage,
                ModelRole::Planner,
                conductor_model,
                harness.repair_prompt(&response, &initial_error),
            )?;
            harness.parse(&repaired).map_err(|repair_error| {
                format!(
                    "initial decision rejected ({initial_error}); repaired decision rejected ({repair_error})"
                )
            })
        }
        Err(error) => Err(format!("conductor decision rejected: {error}")),
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
