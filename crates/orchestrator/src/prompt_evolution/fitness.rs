use super::*;
use std::collections::BTreeSet;

const FITNESS_WINDOW_PER_SPLIT: usize = 12;

pub(super) fn summarize<'a>(
    observations: impl Iterator<Item = &'a PromptEvolutionObservation>,
) -> PromptFitness {
    let mut seen = BTreeSet::new();
    let mut entries = observations.collect::<Vec<_>>();
    entries.reverse();
    entries.retain(|observation| seen.insert(observation.evidence_identity()));
    entries.reverse();
    if entries.len() > FITNESS_WINDOW_PER_SPLIT {
        entries = entries.split_off(entries.len() - FITNESS_WINDOW_PER_SPLIT);
    }
    let runs = entries.len();
    let divisor = runs.max(1) as f64;
    let paired = entries
        .iter()
        .filter(|entry| entry.mode != PromptEvaluationMode::Live)
        .collect::<Vec<_>>();
    let paired_divisor = paired.len().max(1) as f64;
    let step_credits = entries
        .iter()
        .flat_map(|entry| entry.step_credits.iter())
        .map(|step| step.credit.clamp(0.0, 1.0))
        .collect::<Vec<_>>();
    PromptFitness {
        runs,
        paired_runs: paired.len(),
        replay_runs: entries
            .iter()
            .filter(|entry| entry.mode.is_replay())
            .count(),
        execution_runs: entries
            .iter()
            .filter(|entry| entry.mode.is_execution())
            .count(),
        average_reward: entries.iter().map(|entry| entry.reward()).sum::<f64>() / divisor,
        average_relative_reward: paired
            .iter()
            .map(|entry| entry.group_relative_reward())
            .sum::<f64>()
            / paired_divisor,
        average_step_credit: step_credits.iter().sum::<f64>() / step_credits.len().max(1) as f64,
        format_valid_rate: entries.iter().filter(|entry| entry.format_valid).count() as f64
            / divisor,
        success_rate: entries.iter().filter(|entry| entry.succeeded).count() as f64 / divisor,
        average_quality: entries
            .iter()
            .map(|entry| entry.quality_score.clamp(0.0, 1.0))
            .sum::<f64>()
            / divisor,
        average_latency_ms: entries.iter().map(|entry| entry.latency_ms).sum::<u64>() as f64
            / divisor,
        average_total_tokens: entries.iter().map(|entry| entry.total_tokens).sum::<u64>() as f64
            / divisor,
        average_cost_microusd: entries
            .iter()
            .map(|entry| entry.estimated_cost_microusd)
            .sum::<u64>() as f64
            / divisor,
        safety_violations: entries.iter().map(|entry| entry.safety_violations).sum(),
        task_class_coverage: entries
            .iter()
            .map(|entry| entry.task_class.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
    }
}
