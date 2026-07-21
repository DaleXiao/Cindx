use super::*;

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

pub(crate) fn prompt_evaluation_inflight() -> &'static Mutex<BTreeSet<String>> {
    PROMPT_EVALUATIONS_INFLIGHT.get_or_init(|| Mutex::new(BTreeSet::new()))
}

pub(crate) fn wait_for_prompt_evaluation_idle(
    state: &tauri::State<'_, AppState>,
    control: &Arc<AgentRunControl>,
) -> Result<bool, String> {
    let mut idle_since = None::<Instant>;
    loop {
        if control.should_stop() {
            return Ok(false);
        }
        let foreground_active = !state
            .agent_run_controls
            .lock()
            .map_err(|error| format!("agent run control lock poisoned: {error}"))?
            .is_empty();
        if foreground_active {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed()
            >= Duration::from_millis(PROMPT_EVALUATION_IDLE_GRACE_MS)
        {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}
