use crate::app_state::AppState;
use crate::collaboration_service::truncate_for_collaboration;
use crate::collaboration_stage_runtime::CollaborationStageError;
use crate::configuration_models::ProviderConfig;
use crate::prompt_evolution_runtime::prompt_evolution_evaluation_for_run;
use agent_core::{Message, MessageRole, Metadata};
use agent_runtime::AgentRunControl;
use orchestrator::{AgentPolicy, ConductorPromptGenome, PromptEvolutionStrategy};
use std::collections::BTreeSet;

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

pub(super) fn ensure_planning_current(
    cancellation: &AgentRunControl,
) -> Result<(), CollaborationStageError> {
    if cancellation.should_stop() {
        Err(CollaborationStageError::RunStopped)
    } else if cancellation.has_pending_steer() {
        Err(CollaborationStageError::SteerInterrupted)
    } else {
        Ok(())
    }
}

pub(super) fn selected_strategy_profile(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    effort: AgentPolicy,
    run_context: &Metadata,
) -> Result<(ConductorPromptGenome, String), String> {
    #[cfg(feature = "realworld-eval")]
    if let Some(profile) = evaluation_frozen_strategy_profile(effort)? {
        return Ok((profile, "evaluation_frozen_profile".to_string()));
    }
    if !should_evaluate_strategy_profile(effort, config.prompt_evolution_enabled) {
        return Ok((
            ConductorPromptGenome::seed_for_effort(effort.label()),
            "seed_fallback".to_string(),
        ));
    }
    if let Ok(evaluation) = prompt_evolution_evaluation_for_run(state, effort.label(), run_context)
    {
        return Ok((evaluation.next_profile, evaluation.next_mode));
    }
    Ok((
        ConductorPromptGenome::seed_for_effort(effort.label()),
        "seed_fallback".to_string(),
    ))
}

#[cfg(feature = "realworld-eval")]
fn evaluation_frozen_strategy_profile(
    effort: AgentPolicy,
) -> Result<Option<ConductorPromptGenome>, String> {
    let Some(path) = std::env::var_os("CINDX_AGENT_REALWORLD_PROFILE_PATH") else {
        return Ok(None);
    };
    if !matches!(effort, AgentPolicy::Auto | AgentPolicy::Pro) {
        return Err("evaluation frozen profiles are valid only for Auto or Pro".to_string());
    }
    let encoded = std::fs::read(&path).map_err(|error| {
        format!(
            "failed to read evaluation frozen profile {}: {error}",
            std::path::Path::new(&path).display()
        )
    })?;
    let snapshot = orchestrator::FrozenPromptProfileSnapshot::from_json_slice(&encoded)?;
    if snapshot.effort != effort.label() {
        return Err(format!(
            "evaluation frozen profile effort {} does not match {}",
            snapshot.effort,
            effort.label()
        ));
    }
    if let Ok(expected) = std::env::var("CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256") {
        if expected != snapshot.artifact_sha256()? {
            return Err(
                "evaluation frozen profile artifact digest changed after preflight".to_string(),
            );
        }
    }
    Ok(Some(snapshot.genome))
}

pub(crate) fn should_evaluate_strategy_profile(
    effort: AgentPolicy,
    prompt_evolution_enabled: bool,
) -> bool {
    prompt_evolution_enabled && effort.prompt_evolution() != PromptEvolutionStrategy::Disabled
}
