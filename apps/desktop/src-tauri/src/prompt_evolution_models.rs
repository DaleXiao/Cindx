use super::*;
use agent_harness::ExclusiveKeyRegistry;
use std::sync::{Condvar, OnceLock};

static PROMPT_EVALUATION_WAKE: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();

#[derive(Default)]
pub(crate) struct PromptEvolutionAccumulator {
    pub(crate) effort: String,
    pub(crate) generation: u32,
    pub(crate) runs: usize,
    pub(crate) train_runs: usize,
    pub(crate) holdout_runs: usize,
    pub(crate) reflection_runs: usize,
    pub(crate) succeeded: usize,
    pub(crate) reward_total: f64,
    pub(crate) relative_reward_total: f64,
    pub(crate) relative_reward_runs: usize,
    pub(crate) step_credit_total: f64,
    pub(crate) step_credit_count: usize,
    pub(crate) quality_total: f64,
    pub(crate) latency_total_ms: u64,
    pub(crate) token_total: u64,
}

pub(crate) struct PromptEvolutionEvaluation {
    pub(crate) population: Vec<ConductorPromptGenome>,
    pub(crate) observations: Vec<PromptEvolutionObservation>,
    pub(crate) frontier_ids: BTreeSet<String>,
    pub(crate) champion_id: Option<String>,
    pub(crate) champion_score: Option<f64>,
    pub(crate) champion_confidence: Option<PromptPromotionConfidence>,
    pub(crate) status: String,
    pub(crate) freeze_reason: Option<String>,
    pub(crate) stagnant_generations: usize,
    pub(crate) evaluated_generations: usize,
    pub(crate) next_mode: String,
    pub(crate) next_profile: ConductorPromptGenome,
    pub(crate) mutation_parent: Option<ConductorPromptGenome>,
    pub(crate) mutation_trajectories: Vec<AgentEvaluationReflectionPacket>,
}

pub(crate) struct PromptEvolutionReconciliation {
    pub(crate) evaluation: PromptEvolutionEvaluation,
    pub(crate) deployment_recovery_error: Option<String>,
}

pub(crate) struct PromptPairwiseEvaluationOutcome {
    pub(crate) progressed: bool,
    pub(crate) deployment_recovery_error: Option<String>,
}

impl PromptPairwiseEvaluationOutcome {
    pub(crate) fn no_work() -> Self {
        Self {
            progressed: false,
            deployment_recovery_error: None,
        }
    }

    pub(crate) fn progressed(deployment_recovery_error: Option<String>) -> Self {
        Self {
            progressed: true,
            deployment_recovery_error,
        }
    }
}

pub(crate) fn prompt_evaluation_inflight() -> &'static ExclusiveKeyRegistry {
    PROMPT_EVALUATIONS_INFLIGHT
        .get_or_init(|| ExclusiveKeyRegistry::new("prompt evaluation inflight"))
}

pub(crate) fn notify_prompt_evaluation_worker() {
    let (revision, wake) = PROMPT_EVALUATION_WAKE.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let mut revision = revision
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *revision = revision.saturating_add(1);
    wake.notify_one();
}

pub(crate) fn wait_for_prompt_evaluation_worker(observed: &mut u64, timeout: Duration) {
    let (revision, wake) = PROMPT_EVALUATION_WAKE.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let revision = revision
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let revision = if *revision == *observed {
        wake.wait_timeout(revision, timeout)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0
    } else {
        revision
    };
    *observed = *revision;
}
