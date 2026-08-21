use serde::{Deserialize, Serialize};

pub const ADMITTED_DIRECT_JUDGE_TASK_CLASS: &str = "direct_judge_shadow";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptFitness {
    pub runs: usize,
    pub paired_runs: usize,
    pub replay_runs: usize,
    pub execution_runs: usize,
    pub average_reward: f64,
    pub average_relative_reward: f64,
    pub average_step_credit: f64,
    pub format_valid_rate: f64,
    pub success_rate: f64,
    pub average_quality: f64,
    pub average_latency_ms: f64,
    pub average_total_tokens: f64,
    pub average_cost_microusd: f64,
    pub safety_violations: u64,
    pub task_class_coverage: usize,
}

/// Plain admitted-window statistics crossing into prompt evolution. The
/// producing side (agent-application admission contract) has already bound
/// and approved the underlying review receipt; this type carries no digest
/// authority of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedDirectJudgeFitness {
    pub scored_runs: usize,
    pub passed_runs: usize,
    pub average_reward_bps: u16,
}

/// Maps an admitted direct-judge window into the prompt-evolution fitness
/// shape. The mapping is deliberately conservative: no paired, replay, or
/// execution credit, no latency/token/cost claims, and the judge reward
/// stands in as the quality proxy. Promotion never reads PromptFitness, so
/// this admission cannot reach production promotion through fitness alone.
pub fn admitted_direct_judge_fitness_into_prompt_fitness(
    admitted: &AdmittedDirectJudgeFitness,
) -> PromptFitness {
    let runs = admitted.scored_runs;
    let divisor = runs.max(1) as f64;
    let average_reward = f64::from(admitted.average_reward_bps.min(10_000)) / 10_000.0;
    PromptFitness {
        runs,
        paired_runs: 0,
        replay_runs: 0,
        execution_runs: 0,
        average_reward,
        average_relative_reward: 0.0,
        average_step_credit: 0.0,
        format_valid_rate: if runs == 0 { 0.0 } else { 1.0 },
        success_rate: admitted.passed_runs.min(runs) as f64 / divisor,
        average_quality: average_reward,
        average_latency_ms: 0.0,
        average_total_tokens: 0.0,
        average_cost_microusd: 0.0,
        safety_violations: 0,
        task_class_coverage: usize::from(runs > 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admitted_fitness_maps_reward_and_never_claims_paired_or_execution_credit() {
        let admitted = AdmittedDirectJudgeFitness {
            scored_runs: 4,
            passed_runs: 3,
            average_reward_bps: 8_750,
        };
        let fitness = admitted_direct_judge_fitness_into_prompt_fitness(&admitted);
        assert_eq!(fitness.runs, 4);
        assert_eq!(fitness.paired_runs, 0);
        assert_eq!(fitness.replay_runs, 0);
        assert_eq!(fitness.execution_runs, 0);
        assert!((fitness.average_reward - 0.875).abs() < 1e-9);
        assert!((fitness.success_rate - 0.75).abs() < 1e-9);
        assert!((fitness.average_quality - 0.875).abs() < 1e-9);
        assert_eq!(fitness.format_valid_rate, 1.0);
        assert_eq!(fitness.safety_violations, 0);
        assert_eq!(fitness.average_latency_ms, 0.0);
        assert_eq!(fitness.average_total_tokens, 0.0);
        assert_eq!(fitness.average_cost_microusd, 0.0);
        assert_eq!(fitness.task_class_coverage, 1);
    }

    #[test]
    fn admitted_fitness_mapping_is_deterministic_and_bounded() {
        let admitted = AdmittedDirectJudgeFitness {
            scored_runs: 2,
            passed_runs: 2,
            average_reward_bps: 10_000,
        };
        let first = admitted_direct_judge_fitness_into_prompt_fitness(&admitted);
        let second = admitted_direct_judge_fitness_into_prompt_fitness(&admitted);
        assert_eq!(first, second);
        assert_eq!(first.average_reward, 1.0);

        let over = AdmittedDirectJudgeFitness {
            scored_runs: 1,
            passed_runs: 5,
            average_reward_bps: 20_000,
        };
        let fitness = admitted_direct_judge_fitness_into_prompt_fitness(&over);
        assert_eq!(fitness.average_reward, 1.0);
        assert_eq!(fitness.success_rate, 1.0);

        let empty = AdmittedDirectJudgeFitness {
            scored_runs: 0,
            passed_runs: 0,
            average_reward_bps: 0,
        };
        let fitness = admitted_direct_judge_fitness_into_prompt_fitness(&empty);
        assert_eq!(fitness.runs, 0);
        assert_eq!(fitness.average_reward, 0.0);
        assert_eq!(fitness.format_valid_rate, 0.0);
        assert_eq!(fitness.task_class_coverage, 0);
    }
}
