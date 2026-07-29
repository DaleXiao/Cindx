use super::*;

impl AgentRunControl {
    pub fn begin_physical_model_attempt(
        &self,
        model: &str,
        estimated_prompt_tokens: u64,
        max_completion_tokens: u64,
        class: RunStageClass,
    ) -> Result<PhysicalModelAttempt, RunStopReason> {
        let mut state = self.state.lock().expect("run control state poisoned");
        if let Some(reason) = self.refresh_stop_reason_locked(&mut state, Instant::now()) {
            return Err(reason);
        }
        self.begin_physical_model_attempt_locked(
            &mut state,
            model,
            estimated_prompt_tokens,
            max_completion_tokens,
            class,
        )
    }

    pub fn begin_physical_model_attempt_at(
        &self,
        expected_epoch: u64,
        model: &str,
        estimated_prompt_tokens: u64,
        max_completion_tokens: u64,
        class: RunStageClass,
    ) -> Result<Option<PhysicalModelAttempt>, RunStopReason> {
        let mut state = self.state.lock().expect("run control state poisoned");
        if let Some(reason) = self.refresh_stop_reason_locked(&mut state, Instant::now()) {
            return Err(reason);
        }
        if !self.objective_epoch_matches_locked(&state, expected_epoch) {
            return Ok(None);
        }
        self.begin_physical_model_attempt_locked(
            &mut state,
            model,
            estimated_prompt_tokens,
            max_completion_tokens,
            class,
        )
        .map(Some)
    }

    fn begin_physical_model_attempt_locked(
        &self,
        state: &mut RunMutableState,
        model: &str,
        estimated_prompt_tokens: u64,
        max_completion_tokens: u64,
        class: RunStageClass,
    ) -> Result<PhysicalModelAttempt, RunStopReason> {
        match state.resources.reserve(
            self.budget,
            class,
            model,
            estimated_prompt_tokens,
            max_completion_tokens,
        ) {
            Ok(attempt) => Ok(attempt),
            Err(ResourceAdmissionError::ProtectedReserve) => {
                Err(RunStopReason::StageBudgetExhausted)
            }
            Err(ResourceAdmissionError::Exhausted) => {
                state.stop_reason = Some(RunStopReason::ModelResourceBudgetExceeded);
                Err(RunStopReason::ModelResourceBudgetExceeded)
            }
        }
    }

    pub fn finish_physical_model_attempt(
        &self,
        attempt: PhysicalModelAttempt,
        usage: Option<ModelAttemptUsage>,
    ) -> bool {
        let mut state = self.state.lock().expect("run control state poisoned");
        let settlement = state.resources.settle(self.budget, attempt, usage);
        if settlement.exhausted {
            state
                .stop_reason
                .get_or_insert(RunStopReason::ModelResourceBudgetExceeded);
        }
        settlement.settled
    }

    pub fn finish_physical_model_attempt_at(
        &self,
        expected_epoch: u64,
        attempt: PhysicalModelAttempt,
        usage: Option<ModelAttemptUsage>,
    ) -> bool {
        let mut state = self.state.lock().expect("run control state poisoned");
        let settlement = state.resources.settle(self.budget, attempt, usage);
        if settlement.exhausted {
            state
                .stop_reason
                .get_or_insert(RunStopReason::ModelResourceBudgetExceeded);
        }
        settlement.settled
            && state.stop_reason.is_none()
            && !self.user_cancelled.load(Ordering::SeqCst)
            && self.objective_epoch_matches_locked(&state, expected_epoch)
    }

    pub fn resource_usage(&self) -> RunResourceSnapshot {
        self.state
            .lock()
            .expect("run control state poisoned")
            .resources
            .snapshot()
    }
}
