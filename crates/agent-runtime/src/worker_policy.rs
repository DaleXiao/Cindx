use crate::AgentLoopState;
use agent_core::{Message, MessageRole, Metadata};

const WORKER_FINALIZATION_KIND: &str = "worker_finalization";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerTurnPolicy {
    evidence_turns: usize,
    evidence_repair_turns: usize,
    finalization_turns: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerTurnPhase {
    Evidence,
    Finalization,
}

impl WorkerTurnPolicy {
    pub fn isolated_evidence(max_model_turns: usize, has_tools: bool) -> Self {
        Self {
            evidence_turns: max_model_turns.max(1),
            // Tool workers reserve one tool-capable contract-repair turn. It is
            // consumed only when the evidence phase reaches its boundary with
            // an unresolved task obligation, so successful workers pay no
            // additional provider call.
            evidence_repair_turns: usize::from(has_tools),
            // Every isolated worker owns one protected recovery/finalization turn.
            // Tool workers use it to convert gathered evidence into an answer;
            // no-tool workers use it only when the first provider response is
            // empty, truncated, or otherwise unusable. A usable first response
            // still completes immediately and pays no extra model call.
            finalization_turns: 1,
        }
    }

    pub fn runtime_turn_limit(self) -> usize {
        self.evidence_turns
            .saturating_add(self.evidence_repair_turns)
            .saturating_add(self.finalization_turns)
            .max(1)
    }

    pub fn phase_for_turn(self, completed_turns: usize) -> WorkerTurnPhase {
        if self.finalization_turns == 0 || completed_turns < self.evidence_turns {
            WorkerTurnPhase::Evidence
        } else {
            WorkerTurnPhase::Finalization
        }
    }

    pub fn supports_evidence_repair(self) -> bool {
        self.evidence_repair_turns > 0
    }

    pub fn prepare_finalization(self, state: &mut AgentLoopState) {
        if state
            .messages
            .last()
            .and_then(|message| message.metadata.get("kind"))
            .map(String::as_str)
            == Some(WORKER_FINALIZATION_KIND)
        {
            return;
        }
        state.messages.push(Message {
            role: MessageRole::User,
            content: concat!(
                "The evidence phase is complete and tools are now unavailable. ",
                "Do not request more tools. Return the assigned concise work product now, ",
                "grounded only in evidence and context already collected."
            )
            .to_string(),
            metadata: [("kind".to_string(), WORKER_FINALIZATION_KIND.to_string())]
                .into_iter()
                .collect::<Metadata>(),
        });
    }

    pub fn prepare_turn(self, state: &mut AgentLoopState) -> WorkerTurnPhase {
        let phase = self.phase_for_turn(state.turn);
        if phase == WorkerTurnPhase::Finalization {
            self.prepare_finalization(state);
        }
        phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{start_agent_loop, AgentRuntimeConfig};
    use agent_core::TaskId;

    #[test]
    fn tool_workers_reserve_exactly_one_finalization_turn() {
        let policy = WorkerTurnPolicy::isolated_evidence(3, true);
        let mut state = start_agent_loop(
            TaskId("task".to_string()),
            "inspect",
            AgentRuntimeConfig {
                max_turns: policy.runtime_turn_limit(),
            },
        );
        assert_eq!(policy.runtime_turn_limit(), 5);
        assert_eq!(policy.prepare_turn(&mut state), WorkerTurnPhase::Evidence);
        state.turn = 3;
        assert_eq!(
            policy.prepare_turn(&mut state),
            WorkerTurnPhase::Finalization
        );
        let message_count = state.messages.len();
        assert_eq!(
            policy.prepare_turn(&mut state),
            WorkerTurnPhase::Finalization
        );
        assert_eq!(state.messages.len(), message_count);
    }

    #[test]
    fn no_tool_workers_reserve_one_bounded_recovery_turn() {
        let policy = WorkerTurnPolicy::isolated_evidence(1, false);
        let mut state = start_agent_loop(
            TaskId("task".to_string()),
            "answer",
            AgentRuntimeConfig {
                max_turns: policy.runtime_turn_limit(),
            },
        );

        assert_eq!(policy.runtime_turn_limit(), 2);
        assert_eq!(policy.prepare_turn(&mut state), WorkerTurnPhase::Evidence);
        state.turn = 1;
        assert_eq!(
            policy.prepare_turn(&mut state),
            WorkerTurnPhase::Finalization
        );
    }
}
