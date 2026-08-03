use agent_runtime::{AgentLoopState, OutcomeClaimDecision};

pub(crate) fn observe_candidate<T, E>(
    runtime: &mut AgentLoopState,
    steer_epoch: u64,
    answer: &str,
    completion_gate: &Result<Option<T>, E>,
) {
    let decision = match completion_gate {
        Ok(Some(_)) => OutcomeClaimDecision::RepairRequired,
        Ok(None) => OutcomeClaimDecision::Accepted,
        Err(_) => OutcomeClaimDecision::ContractFailed,
    };
    runtime
        .task_contract
        .observe_completion_candidate(steer_epoch, runtime.turn, answer, decision);
}
