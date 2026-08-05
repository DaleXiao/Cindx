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

    /// Creates an equal, independently metered treatment lane. Only the
    /// parent's cancellation signal is shared; counters, deadlines, stage
    /// usage, results, and mutable progress remain isolated.
    pub fn isolated_treatment(&self, allocation_divisor: usize) -> Result<Self, RunStopReason> {
        let allocation_divisor = allocation_divisor.max(1);
        let now = Instant::now();
        let mut state = self.state.lock().expect("run control state poisoned");
        if let Some(reason) = self.refresh_stop_reason_locked(&mut state, now) {
            return Err(reason);
        }
        let resources = state.resources.snapshot().segment;
        let model_calls = self.model_calls.load(Ordering::SeqCst);
        let tool_calls = self.tool_calls.load(Ordering::SeqCst);
        let agent_turns = self.agent_turns.load(Ordering::SeqCst);
        let remaining_duration = self
            .budget
            .max_duration
            .saturating_sub(now.duration_since(state.started_at));
        let remaining_model_calls = self.budget.max_model_calls.saturating_sub(model_calls);
        let remaining_tool_calls = self.budget.max_tool_calls.saturating_sub(tool_calls);
        let remaining_agent_turns = self.budget.max_agent_turns.saturating_sub(agent_turns);
        let unlocked_model_calls = state
            .model_call_limit
            .saturating_sub(model_calls)
            .min(remaining_model_calls);
        let unlocked_tool_calls = state
            .tool_call_limit
            .saturating_sub(tool_calls)
            .min(remaining_tool_calls);
        let unlocked_agent_turns = state
            .agent_turn_limit
            .saturating_sub(agent_turns)
            .min(remaining_agent_turns);
        let remaining_repairs = self
            .budget
            .max_repair_attempts
            .saturating_sub(self.repair_attempts.load(Ordering::SeqCst));
        let remaining_tokens = self.budget.max_total_tokens.saturating_sub(
            resources
                .total_tokens
                .saturating_add(resources.reserved_tokens),
        );
        let remaining_physical_attempts = self
            .budget
            .max_physical_model_attempts
            .saturating_sub(resources.physical_attempts.try_into().unwrap_or(usize::MAX));
        if remaining_duration == Duration::ZERO
            || remaining_model_calls == 0
            || remaining_tool_calls == 0
            || remaining_agent_turns == 0
            || remaining_repairs == 0
            || remaining_tokens == 0
            || remaining_physical_attempts == 0
        {
            return Err(RunStopReason::StageBudgetExhausted);
        }
        let divide = |value: usize| value / allocation_divisor;
        let divide_tokens = |value: u64| value / allocation_divisor as u64;
        if divide(remaining_model_calls) == 0
            || divide(remaining_tool_calls) == 0
            || divide(remaining_agent_turns) == 0
            || divide(unlocked_model_calls) == 0
            || divide(unlocked_tool_calls) == 0
            || divide(unlocked_agent_turns) == 0
            || divide(remaining_repairs) == 0
            || divide_tokens(remaining_tokens) == 0
            || divide(remaining_physical_attempts) == 0
        {
            return Err(RunStopReason::StageBudgetExhausted);
        }
        let mut budget = self.budget;
        budget.max_duration = remaining_duration;
        budget.initial_model_calls = divide(unlocked_model_calls);
        budget.max_model_calls = divide(remaining_model_calls);
        budget.model_calls_per_extension = divide(budget.model_calls_per_extension).max(1);
        budget.initial_tool_calls = divide(unlocked_tool_calls);
        budget.max_tool_calls = divide(remaining_tool_calls);
        budget.tool_calls_per_extension = divide(budget.tool_calls_per_extension).max(1);
        budget.initial_agent_turns = divide(unlocked_agent_turns);
        budget.max_agent_turns = divide(remaining_agent_turns);
        budget.agent_turns_per_extension = divide(budget.agent_turns_per_extension).max(1);
        budget.max_repair_attempts = divide(remaining_repairs);
        budget.terminal_model_call_reserve = divide(budget.terminal_model_call_reserve)
            .min(budget.max_model_calls.saturating_sub(1));
        budget.max_total_tokens = divide_tokens(remaining_tokens);
        budget.max_physical_model_attempts = divide(remaining_physical_attempts);
        budget.terminal_token_reserve = divide_tokens(budget.terminal_token_reserve)
            .min(budget.max_total_tokens.saturating_sub(1));
        budget.terminal_physical_model_attempt_reserve =
            divide(budget.terminal_physical_model_attempt_reserve)
                .min(budget.max_physical_model_attempts.saturating_sub(1));
        let mut child = Self::with_budget_at_steer_epoch(budget, self.steer_epoch());
        child.parent_cancelled = Some(Arc::clone(&self.user_cancelled));
        Ok(child)
    }

    pub fn absorb_isolated_treatments(
        &self,
        treatments: &[&AgentRunControl],
    ) -> Result<(), RunStopReason> {
        let mut state = self.state.lock().expect("run control state poisoned");
        let mut model_call_extensions = 0usize;
        let mut tool_call_extensions = 0usize;
        let mut agent_turn_extensions = 0usize;
        let acquired_extension = |limit: usize, initial: usize, max: usize| {
            limit.saturating_sub(initial.min(max).max(1))
        };
        for treatment in treatments {
            if treatment
                .parent_cancelled
                .as_ref()
                .is_none_or(|cancelled| !Arc::ptr_eq(cancelled, &self.user_cancelled))
            {
                state
                    .stop_reason
                    .get_or_insert(RunStopReason::ProviderUnavailable);
                return Err(RunStopReason::ProviderUnavailable);
            }
            let snapshot = treatment.snapshot();
            model_call_extensions = model_call_extensions.saturating_add(acquired_extension(
                snapshot.model_call_limit,
                snapshot.budget.initial_model_calls,
                snapshot.budget.max_model_calls,
            ));
            tool_call_extensions = tool_call_extensions.saturating_add(acquired_extension(
                snapshot.tool_call_limit,
                snapshot.budget.initial_tool_calls,
                snapshot.budget.max_tool_calls,
            ));
            agent_turn_extensions = agent_turn_extensions.saturating_add(acquired_extension(
                snapshot.agent_turn_limit,
                snapshot.budget.initial_agent_turns,
                snapshot.budget.max_agent_turns,
            ));
            state.budget_extensions = state
                .budget_extensions
                .saturating_add(snapshot.budget_extensions);
            self.model_calls
                .fetch_add(snapshot.model_calls, Ordering::SeqCst);
            self.tool_calls
                .fetch_add(snapshot.tool_calls, Ordering::SeqCst);
            self.agent_turns
                .fetch_add(snapshot.agent_turns, Ordering::SeqCst);
            self.repair_attempts
                .fetch_add(snapshot.repair_attempts, Ordering::SeqCst);
            state.resources.absorb_completed_segment(snapshot.resources);
        }
        let model_calls = self.model_calls.load(Ordering::SeqCst);
        let tool_calls = self.tool_calls.load(Ordering::SeqCst);
        let agent_turns = self.agent_turns.load(Ordering::SeqCst);
        state.model_call_limit = state
            .model_call_limit
            .saturating_add(model_call_extensions)
            .max(model_calls)
            .min(self.budget.max_model_calls);
        state.tool_call_limit = state
            .tool_call_limit
            .saturating_add(tool_call_extensions)
            .max(tool_calls)
            .min(self.budget.max_tool_calls);
        state.agent_turn_limit = state
            .agent_turn_limit
            .saturating_add(agent_turn_extensions)
            .max(agent_turns)
            .min(self.budget.max_agent_turns);
        let resources = state.resources.snapshot().segment;
        let exceeded = model_calls > self.budget.max_model_calls
            || tool_calls > self.budget.max_tool_calls
            || agent_turns > self.budget.max_agent_turns
            || self.repair_attempts.load(Ordering::SeqCst) > self.budget.max_repair_attempts
            || resources.physical_attempts
                > u64::try_from(self.budget.max_physical_model_attempts).unwrap_or(u64::MAX)
            || resources
                .total_tokens
                .saturating_add(resources.reserved_tokens)
                > self.budget.max_total_tokens;
        state.last_progress_at = Instant::now();
        if exceeded {
            state
                .stop_reason
                .get_or_insert(RunStopReason::ModelResourceBudgetExceeded);
            return Err(RunStopReason::ModelResourceBudgetExceeded);
        }
        Ok(())
    }

    fn with_budget_at_steer_epoch(budget: RunBudget, applied_epoch: u64) -> Self {
        let now = Instant::now();
        Self {
            budget,
            user_cancelled: Arc::new(AtomicBool::new(false)),
            parent_cancelled: None,
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
                goal_delta_fingerprints: BTreeSet::new(),
                goal_delta_count: 0,
                model_extension_goal_delta: 0,
                tool_extension_goal_delta: 0,
                agent_turn_extension_goal_delta: 0,
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
