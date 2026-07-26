use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunStageClass {
    Conductor,
    Candidate,
    Worker,
    Reviewer,
    Synthesizer,
    Repair,
    Finalizer,
    Other,
}

impl RunStageClass {
    pub fn from_label(label: &str) -> Self {
        let label = label.to_ascii_lowercase();
        if label.contains("terminal_executor")
            || label.contains("finalizer")
            || label.contains("user_delivery")
            || label.contains("deliver_to_user")
        {
            Self::Finalizer
        } else if label.contains("repair") || label.contains("recover") || label.contains("retry") {
            Self::Repair
        } else if label.contains("conductor") || label.contains("planner") {
            Self::Conductor
        } else if label.contains("candidate") {
            Self::Candidate
        } else if label.contains("review") || label.contains("critic") {
            Self::Reviewer
        } else if label.contains("synth") || label.contains("summar") || label.contains("final") {
            Self::Synthesizer
        } else if label.contains("worker") {
            Self::Worker
        } else {
            Self::Other
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Reviewer | Self::Synthesizer | Self::Repair | Self::Finalizer
        )
    }

    pub fn is_finalizer(self) -> bool {
        self == Self::Finalizer
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunStageBudget {
    pub max_model_calls: usize,
    pub max_duration: Duration,
    pub terminal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBudget {
    pub max_duration: Duration,
    pub model_call_timeout: Duration,
    pub tool_call_timeout: Duration,
    pub initial_model_calls: usize,
    pub max_model_calls: usize,
    pub model_calls_per_extension: usize,
    pub initial_tool_calls: usize,
    pub max_tool_calls: usize,
    pub tool_calls_per_extension: usize,
    pub no_progress_timeout: Duration,
    pub max_identical_actions: usize,
    pub initial_agent_turns: usize,
    pub max_agent_turns: usize,
    pub agent_turns_per_extension: usize,
    pub max_repair_attempts: usize,
    pub terminal_model_call_reserve: usize,
    pub terminal_time_reserve: Duration,
}

impl RunBudget {
    pub fn for_effort(effort: &str) -> Self {
        match effort {
            "fast" => Self {
                max_duration: Duration::from_secs(5 * 60),
                model_call_timeout: Duration::from_secs(3 * 60),
                tool_call_timeout: Duration::from_secs(5 * 60),
                initial_model_calls: 6,
                max_model_calls: 12,
                model_calls_per_extension: 3,
                initial_tool_calls: 12,
                max_tool_calls: 24,
                tool_calls_per_extension: 6,
                no_progress_timeout: Duration::from_secs(90),
                max_identical_actions: 3,
                initial_agent_turns: 6,
                max_agent_turns: 12,
                agent_turns_per_extension: 3,
                max_repair_attempts: 2,
                terminal_model_call_reserve: 2,
                terminal_time_reserve: Duration::from_secs(3 * 60),
            },
            "pro" => Self {
                max_duration: Duration::from_secs(4 * 60 * 60),
                model_call_timeout: Duration::from_secs(15 * 60),
                tool_call_timeout: Duration::from_secs(60 * 60),
                initial_model_calls: 48,
                max_model_calls: 384,
                model_calls_per_extension: 48,
                initial_tool_calls: 96,
                max_tool_calls: 768,
                tool_calls_per_extension: 96,
                no_progress_timeout: Duration::from_secs(5 * 60),
                max_identical_actions: 4,
                initial_agent_turns: 48,
                max_agent_turns: 384,
                agent_turns_per_extension: 48,
                max_repair_attempts: 8,
                terminal_model_call_reserve: 8,
                terminal_time_reserve: Duration::from_secs(15 * 60),
            },
            _ => Self {
                max_duration: Duration::from_secs(45 * 60),
                model_call_timeout: Duration::from_secs(5 * 60),
                tool_call_timeout: Duration::from_secs(15 * 60),
                initial_model_calls: 18,
                max_model_calls: 72,
                model_calls_per_extension: 18,
                initial_tool_calls: 36,
                max_tool_calls: 144,
                tool_calls_per_extension: 36,
                no_progress_timeout: Duration::from_secs(2 * 60),
                max_identical_actions: 3,
                initial_agent_turns: 18,
                max_agent_turns: 72,
                agent_turns_per_extension: 18,
                max_repair_attempts: 4,
                terminal_model_call_reserve: 4,
                terminal_time_reserve: Duration::from_secs(5 * 60),
            },
        }
    }

    /// The finalizer owns the complete terminal reserve. Internal review and
    /// synthesis may run before that boundary, but cannot consume the only
    /// model-call window available for the user-facing answer.
    pub fn finalizer_model_call_reserve(self) -> usize {
        self.terminal_model_call_reserve.max(1)
    }

    pub fn finalizer_time_reserve(self) -> Duration {
        self.terminal_time_reserve
    }

    pub fn protected_model_call_reserve(self, class: RunStageClass) -> usize {
        if class.is_finalizer() {
            0
        } else if class.is_terminal() {
            self.finalizer_model_call_reserve()
        } else {
            self.terminal_model_call_reserve
        }
    }

    pub fn protected_time_reserve(self, class: RunStageClass) -> Duration {
        if class.is_finalizer() {
            Duration::ZERO
        } else if class.is_terminal() {
            self.finalizer_time_reserve()
        } else {
            self.terminal_time_reserve
        }
    }

    pub fn stage_budget(self, class: RunStageClass) -> RunStageBudget {
        let (max_model_calls, duration_divisor) = match class {
            RunStageClass::Conductor => (self.max_repair_attempts.saturating_add(1), 5),
            RunStageClass::Candidate => (self.max_model_calls.saturating_div(3).max(2), 2),
            RunStageClass::Worker => (self.max_model_calls.saturating_div(2).max(2), 1),
            RunStageClass::Reviewer => (self.max_repair_attempts.saturating_add(2), 3),
            RunStageClass::Synthesizer => (self.max_repair_attempts.saturating_add(2), 2),
            RunStageClass::Repair => (self.max_repair_attempts, 4),
            RunStageClass::Finalizer => (self.finalizer_model_call_reserve(), 1),
            RunStageClass::Other => (self.max_model_calls, 1),
        };
        RunStageBudget {
            max_model_calls: max_model_calls.min(self.max_model_calls).max(1),
            max_duration: if class.is_finalizer() {
                self.finalizer_time_reserve().max(Duration::from_millis(1))
            } else {
                self.max_duration / duration_divisor
            },
            terminal: class.is_terminal(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_distinguish_internal_synthesis_from_user_delivery() {
        assert_eq!(
            RunStageClass::from_label("terminal_executor"),
            RunStageClass::Finalizer
        );
        assert_eq!(
            RunStageClass::from_label("final_synthesis"),
            RunStageClass::Synthesizer
        );
    }

    #[test]
    fn finalizer_owns_a_complete_model_call_window() {
        for effort in ["fast", "auto", "pro"] {
            let budget = RunBudget::for_effort(effort);
            assert!(budget.finalizer_model_call_reserve() > 0);
            assert!(budget.finalizer_model_call_reserve() <= budget.terminal_model_call_reserve);
            assert!(budget.finalizer_time_reserve() > Duration::ZERO);
            assert_eq!(
                budget.finalizer_time_reserve(),
                budget.terminal_time_reserve
            );
            assert!(budget.finalizer_time_reserve() >= budget.model_call_timeout);
        }
    }
}
