use agent_core::ToolSpec;
use agent_runtime::{
    AgentAdvance, AgentKernel, AgentLoopState, GroundedCompletionDecision,
    GroundedCompletionReceipt,
};
use model_provider::ModelResponse;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionRejection {
    Repair,
    ContractFailure,
}

pub(crate) struct GroundedModelResponse {
    pub advance: AgentAdvance,
    pub verification_required: bool,
    pub receipt: Option<GroundedCompletionReceipt>,
    pub rejection: Option<CompletionRejection>,
}

pub(crate) fn advance_grounded_model_response(
    runtime: &mut AgentLoopState,
    tools: &[ToolSpec],
    response: ModelResponse,
    previous_message_count: usize,
    steer_epoch: u64,
    visible_evidence_sequences: &[u64],
) -> GroundedModelResponse {
    let mut advance = AgentKernel::new(runtime, tools).advance_model_response(response);
    let mut verification_required = false;
    let mut receipt = None;
    let mut rejection = None;

    if let AgentAdvance::Completed { answer } = &advance {
        match AgentKernel::new(runtime, tools).decide_grounded_completion(
            steer_epoch,
            answer,
            visible_evidence_sequences,
        ) {
            Ok(GroundedCompletionDecision::Repair(instruction)) => {
                runtime.messages.truncate(previous_message_count);
                AgentKernel::new(runtime, tools).apply_instruction(&instruction);
                verification_required = true;
                rejection = Some(CompletionRejection::Repair);
            }
            Ok(GroundedCompletionDecision::Deliver(completion_receipt)) => {
                receipt = Some(completion_receipt);
            }
            Err(failure) => {
                runtime.messages.truncate(previous_message_count);
                advance = AgentAdvance::Failed { failure };
                rejection = Some(CompletionRejection::ContractFailure);
            }
        }
    }
    if let AgentAdvance::Retry { instruction } = &advance {
        AgentKernel::new(runtime, tools).apply_model_response_retry(instruction.clone());
    }

    GroundedModelResponse {
        advance,
        verification_required,
        receipt,
        rejection,
    }
}

pub(crate) fn should_reset_rejected_completion_stream(
    visible_stream: bool,
    streamed_output: bool,
    rejection: Option<CompletionRejection>,
) -> bool {
    visible_stream && streamed_output && rejection.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_grounded_candidates_are_withdrawn_from_the_visible_stream() {
        for rejection in [
            CompletionRejection::Repair,
            CompletionRejection::ContractFailure,
        ] {
            assert!(should_reset_rejected_completion_stream(
                true,
                true,
                Some(rejection)
            ));
        }
        assert!(!should_reset_rejected_completion_stream(
            false,
            true,
            Some(CompletionRejection::Repair)
        ));
        assert!(!should_reset_rejected_completion_stream(true, true, None));
    }
}
