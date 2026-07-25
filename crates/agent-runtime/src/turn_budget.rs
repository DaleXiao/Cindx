use crate::AgentLoopState;
use agent_core::MessageRole;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnBudgetExhausted {
    pub completed_turns: usize,
    pub max_turns: usize,
    pub partial_answer: Option<String>,
}

pub(crate) fn ensure_model_turn_available(
    state: &AgentLoopState,
) -> Result<(), AgentTurnBudgetExhausted> {
    if state.turn < state.max_turns {
        Ok(())
    } else {
        Err(turn_budget_exhaustion(state, None))
    }
}

pub(crate) fn record_model_response(state: &mut AgentLoopState) {
    state.turn = state.turn.saturating_add(1);
}

pub(crate) fn turn_budget_exhaustion(
    state: &AgentLoopState,
    latest_partial: Option<String>,
) -> AgentTurnBudgetExhausted {
    AgentTurnBudgetExhausted {
        completed_turns: state.turn.min(state.max_turns),
        max_turns: state.max_turns,
        partial_answer: latest_partial.or_else(|| best_partial_answer(state)),
    }
}

fn best_partial_answer(state: &AgentLoopState) -> Option<String> {
    state
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == MessageRole::Assistant)
        .find_map(|message| {
            let content = message.content.trim();
            (!content.is_empty()).then(|| content.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{start_agent_loop, AgentRuntimeConfig};
    use agent_core::{Message, Metadata, TaskId};

    #[test]
    fn budget_is_rejected_before_an_extra_model_call() {
        let mut state = start_agent_loop(
            TaskId("budget".to_string()),
            "finish once",
            AgentRuntimeConfig { max_turns: 1 },
        );
        record_model_response(&mut state);

        let exhausted = ensure_model_turn_available(&state).expect_err("second call is blocked");
        assert_eq!(exhausted.completed_turns, 1);
        assert_eq!(exhausted.max_turns, 1);
    }

    #[test]
    fn exhaustion_preserves_the_latest_substantive_partial() {
        let mut state = start_agent_loop(
            TaskId("partial".to_string()),
            "continue",
            AgentRuntimeConfig { max_turns: 1 },
        );
        state.turn = 1;
        state.messages.push(Message {
            role: MessageRole::Assistant,
            content: "verified partial".to_string(),
            metadata: Metadata::new(),
        });

        let exhausted = ensure_model_turn_available(&state).expect_err("budget is exhausted");
        assert_eq!(
            exhausted.partial_answer.as_deref(),
            Some("verified partial")
        );
    }
}
