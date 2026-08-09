use crate::{
    collaboration_execution::collaboration_candidate_models, configuration_models::ProviderConfig,
    prompt_pairwise_runtime::schedule_prompt_pairwise_evaluation,
    prompt_profile_serving::restore_prompt_profile_selection,
};
use agent_core::{Metadata, TaskId};
use orchestrator::{AgentPolicy, ConductorPromptGenome};

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalPromptEvaluationSchedule {
    effort: String,
    policy: String,
    worker_models: Vec<String>,
    agent_budget: usize,
    current_profile: ConductorPromptGenome,
}

pub(crate) fn schedule_terminal_prompt_pairwise_evaluation(
    app: &tauri::AppHandle,
    task_id: &TaskId,
    config: &ProviderConfig,
    run_context: &Metadata,
) -> Result<bool, String> {
    let Some(schedule) = terminal_prompt_evaluation_schedule(config, run_context)? else {
        return Ok(false);
    };
    schedule_prompt_pairwise_evaluation(
        app.clone(),
        task_id.clone(),
        run_context.clone(),
        schedule.effort,
        schedule.policy,
        schedule.worker_models,
        schedule.agent_budget,
        schedule.current_profile,
    )?;
    Ok(true)
}

fn terminal_prompt_evaluation_schedule(
    config: &ProviderConfig,
    run_context: &Metadata,
) -> Result<Option<TerminalPromptEvaluationSchedule>, String> {
    if !config.prompt_evolution_enabled || !config.is_ready() {
        return Ok(None);
    }
    let Some(effort) = run_context
        .get("agent_effort")
        .and_then(|value| AgentPolicy::parse_persisted(value))
    else {
        return Ok(None);
    };
    if !effort.uses_conductor() {
        return Ok(None);
    }
    let Some(project_id) = run_context
        .get("project_id")
        .map(String::as_str)
        .map(str::trim)
        .filter(|project_id| !project_id.is_empty())
    else {
        return Ok(None);
    };
    if run_context
        .get("project_root")
        .map(String::as_str)
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return Ok(None);
    }
    let selection = restore_prompt_profile_selection(effort.label(), project_id, run_context)?
        .ok_or_else(|| {
            "completed agent run is missing its prompt profile assignment".to_string()
        })?;
    let agent_budget = effort.max_parallelism();
    let worker_models = collaboration_candidate_models(config, agent_budget);
    if worker_models.is_empty() {
        return Ok(None);
    }
    let policy = run_context
        .get("collaboration_policy")
        .map(String::as_str)
        .map(str::trim)
        .filter(|policy| !policy.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| effort.requested_policy().label().to_string());
    Ok(Some(TerminalPromptEvaluationSchedule {
        effort: effort.label().to_string(),
        policy,
        worker_models,
        agent_budget,
        current_profile: selection.genome,
    }))
}

#[cfg(test)]
#[path = "prompt_terminal_learning_runtime_tests.rs"]
mod tests;
