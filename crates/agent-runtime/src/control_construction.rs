use super::*;

impl AgentRunControl {
    pub fn new(effort: &str) -> Self {
        Self::new_at_steer_epoch(effort, 0)
    }

    pub fn new_at_steer_epoch(effort: &str, applied_epoch: u64) -> Self {
        Self::with_budget_at_steer_epoch(RunBudget::for_effort(effort), applied_epoch)
    }

    pub fn new_at_steer_epoch_with_resource_snapshot(
        effort: &str,
        applied_epoch: u64,
        resources: RunResourceSnapshot,
    ) -> Self {
        let mut control =
            Self::with_budget_at_steer_epoch(RunBudget::for_effort(effort), applied_epoch);
        control
            .state
            .get_mut()
            .expect("run control state poisoned")
            .resources = RunResourceLedger::from_persisted_snapshot(resources);
        control
    }

    pub fn new_for_continuation_at_steer_epoch_with_resource_snapshot(
        effort: &str,
        applied_epoch: u64,
        resources: RunResourceSnapshot,
    ) -> Self {
        let mut control =
            Self::with_budget_at_steer_epoch(RunBudget::for_effort(effort), applied_epoch);
        let mut resources = RunResourceLedger::from_persisted_snapshot(resources);
        resources.start_new_segment();
        control
            .state
            .get_mut()
            .expect("run control state poisoned")
            .resources = resources;
        control
    }

    pub fn with_budget(budget: RunBudget) -> Self {
        Self::with_budget_at_steer_epoch(budget, 0)
    }

    fn with_budget_at_steer_epoch(budget: RunBudget, applied_epoch: u64) -> Self {
        let now = Instant::now();
        Self {
            budget,
            user_cancelled: AtomicBool::new(false),
            model_calls: AtomicUsize::new(0),
            tool_calls: AtomicUsize::new(0),
            agent_turns: AtomicUsize::new(0),
            repair_attempts: AtomicUsize::new(0),
            steer_epoch: AtomicU64::new(applied_epoch),
            state: Mutex::new(RunMutableState {
                started_at: now,
                last_progress_at: now,
                stage: "starting".to_string(),
                detail: String::new(),
                partial_output: String::new(),
                action_history: BTreeMap::new(),
                recent_actions: BTreeMap::new(),
                observation_fingerprints: BTreeSet::new(),
                observation_count: 0,
                checkpoint_fingerprints: BTreeSet::new(),
                checkpoint_count: 0,
                model_extension_checkpoint: 0,
                tool_extension_checkpoint: 0,
                agent_turn_extension_checkpoint: 0,
                model_call_limit: budget
                    .initial_model_calls
                    .min(budget.max_model_calls)
                    .max(1),
                tool_call_limit: budget.initial_tool_calls.min(budget.max_tool_calls).max(1),
                agent_turn_limit: budget
                    .initial_agent_turns
                    .min(budget.max_agent_turns)
                    .max(1),
                budget_extensions: 0,
                stop_reason: None,
                active_model_calls: 0,
                active_tool_calls: 0,
                pending_steers: VecDeque::new(),
                applied_steer_epoch: applied_epoch,
                phase: RunPhase::Executing,
                stage_usage: BTreeMap::new(),
                results: ResultFrontier::default(),
                resources: RunResourceLedger::new(),
            }),
        }
    }
}
