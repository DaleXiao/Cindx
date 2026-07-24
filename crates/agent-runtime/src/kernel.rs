use crate::{
    advance_with_model_response, append_internal_instruction, append_steering_instruction,
    append_tool_observation, completion_verification_instruction,
    interaction_completion_verification_instruction, model_request_for_turn_with_context_budget,
    record_tool_outcome_with_risk, repeated_tool_failure_count, tool_invocation_from_request,
    AgentAdvance, AgentLoopState, AgentTaskStateSnapshot, AgentToolRequest, ContextGovernorReport,
};
use agent_core::{Metadata, ToolInvocation, ToolOutcomeStatus, ToolRisk, ToolSpec};
use model_provider::{ModelRequest, ModelResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKernelInstructionKind {
    CompletionVerification,
    ModelResponseRetry,
}

impl AgentKernelInstructionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompletionVerification => "completion_verification_gate",
            Self::ModelResponseRetry => "model_response_retry",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentKernelInstruction {
    pub kind: AgentKernelInstructionKind,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedAgentTurn {
    pub request: ModelRequest,
    pub context: ContextGovernorReport,
}

/// Typed façade over the pure agent state machine.
///
/// It keeps state transitions that must happen together atomic while the
/// existing free functions remain available for compatibility during the
/// desktop adapter migration.
pub struct AgentKernel<'state, 'tools> {
    state: &'state mut AgentLoopState,
    tools: &'tools [ToolSpec],
}

impl<'state, 'tools> AgentKernel<'state, 'tools> {
    pub fn new(state: &'state mut AgentLoopState, tools: &'tools [ToolSpec]) -> Self {
        Self { state, tools }
    }

    pub fn state(&self) -> &AgentLoopState {
        self.state
    }

    pub fn state_mut(&mut self) -> &mut AgentLoopState {
        self.state
    }

    pub fn snapshot(&self) -> AgentTaskStateSnapshot {
        AgentTaskStateSnapshot::capture(self.state)
    }

    pub fn prepare_model_turn(
        &self,
        user_instructions: Option<&str>,
        runtime_context: Option<&str>,
        context_window_tokens: u64,
        max_output_tokens: u64,
    ) -> PreparedAgentTurn {
        let (request, context) = model_request_for_turn_with_context_budget(
            self.state,
            self.tools,
            user_instructions,
            runtime_context,
            context_window_tokens,
            max_output_tokens,
        );
        PreparedAgentTurn { request, context }
    }

    pub fn advance_model_response(&mut self, response: ModelResponse) -> AgentAdvance {
        advance_with_model_response(self.state, response, self.tools)
    }

    pub fn apply_steer(&mut self, instruction: impl Into<String>, metadata: Metadata) {
        append_steering_instruction(self.state, instruction, metadata);
    }

    pub fn apply_instruction(&mut self, instruction: &AgentKernelInstruction) {
        append_internal_instruction(self.state, instruction.kind.as_str(), &instruction.content);
    }

    pub fn apply_model_response_retry(&mut self, instruction: impl Into<String>) {
        self.apply_instruction(&AgentKernelInstruction {
            kind: AgentKernelInstructionKind::ModelResponseRetry,
            content: instruction.into(),
        });
    }

    pub fn completion_gate(
        &mut self,
        workspace_verification_required: bool,
    ) -> Option<AgentKernelInstruction> {
        let content = interaction_completion_verification_instruction(self.state, self.tools)
            .or_else(|| {
                completion_verification_instruction(
                    self.state,
                    workspace_verification_required,
                    self.tools,
                )
            })?;
        Some(AgentKernelInstruction {
            kind: AgentKernelInstructionKind::CompletionVerification,
            content,
        })
    }

    pub fn repeated_tool_failure_count(&self, request: &AgentToolRequest) -> usize {
        repeated_tool_failure_count(self.state, &request.tool_name, &request.input)
    }

    pub fn tool_invocation(&self, request: &AgentToolRequest) -> ToolInvocation {
        tool_invocation_from_request(&self.state.task_id, request)
    }

    pub fn apply_tool_observation(
        &mut self,
        request: &AgentToolRequest,
        status: &ToolOutcomeStatus,
        risk: Option<&ToolRisk>,
        observation: &str,
    ) {
        record_tool_outcome_with_risk(self.state, &request.tool_name, &request.input, status, risk);
        append_tool_observation(self.state, request.call_id.clone(), observation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{start_agent_loop, AgentRuntimeConfig};
    use agent_core::{Message, MessageRole, TaskId, ToolCallId};

    fn read_tool() -> ToolSpec {
        ToolSpec::builtin(
            "file.read",
            "file",
            "Read a file",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )
    }

    fn request() -> AgentToolRequest {
        AgentToolRequest {
            call_id: ToolCallId("call-1".to_string()),
            tool_name: "file.read".to_string(),
            input: r#"{"path":"README.md"}"#.to_string(),
        }
    }

    #[test]
    fn prepares_budgeted_turn_without_mutating_runtime() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        let tools = vec![read_tool()];
        let prepared = AgentKernel::new(&mut state, &tools).prepare_model_turn(
            Some("Be concise"),
            Some("workspace=/tmp/example"),
            16_384,
            2_048,
        );

        assert_eq!(prepared.request.tools, tools);
        assert_eq!(prepared.request.metadata["agent_turn"], "0");
        assert_eq!(state.turn, 0);
    }

    #[test]
    fn tool_outcome_and_observation_are_applied_atomically() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        let tools = vec![read_tool()];
        let request = request();
        AgentKernel::new(&mut state, &tools).apply_tool_observation(
            &request,
            &ToolOutcomeStatus::Failed,
            Some(&ToolRisk::ReadOnly),
            "tool=file.read\nstatus=failed\noutput=missing",
        );

        assert_eq!(
            AgentKernel::new(&mut state, &tools).repeated_tool_failure_count(&request),
            1
        );
        assert!(matches!(
            state.messages.last(),
            Some(Message {
                role: MessageRole::Tool,
                ..
            })
        ));
        assert_eq!(
            state.messages.last().unwrap().metadata["tool_call_id"],
            "call-1"
        );
    }

    #[test]
    fn completion_gate_returns_typed_instruction() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "change the workspace",
            AgentRuntimeConfig::default(),
        );
        state.successful_mutations = 1;
        let tools = vec![read_tool()];
        let instruction = AgentKernel::new(&mut state, &tools)
            .completion_gate(true)
            .expect("verification should be required");

        assert_eq!(
            instruction.kind,
            AgentKernelInstructionKind::CompletionVerification
        );
        assert!(instruction.content.contains("post-change verification"));
    }
}
