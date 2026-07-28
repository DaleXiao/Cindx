use crate::{
    advance_with_model_response, append_internal_instruction, append_steering_instruction,
    append_tool_observation, model_request_for_turn_with_context_budget,
    record_tool_outcome_with_risk, repeated_tool_failure_count, tool_invocation_from_request,
    AgentAdvance, AgentLoopState, AgentTaskStateSnapshot, AgentToolRequest,
    AgentTurnBudgetExhausted, ContextGovernorReport, ContextInvariantViolation,
    WorkspaceVerificationPolicy,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTurnPreparationError {
    Budget(AgentTurnBudgetExhausted),
    Context(ContextInvariantViolation),
}

impl std::fmt::Display for AgentTurnPreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Budget(exhausted) => write!(
                formatter,
                "agent turn budget exhausted after {} of {} turns",
                exhausted.completed_turns, exhausted.max_turns
            ),
            Self::Context(violation) => write!(formatter, "{violation}"),
        }
    }
}

impl std::error::Error for AgentTurnPreparationError {}

impl From<AgentTurnBudgetExhausted> for AgentTurnPreparationError {
    fn from(exhausted: AgentTurnBudgetExhausted) -> Self {
        Self::Budget(exhausted)
    }
}

impl From<ContextInvariantViolation> for AgentTurnPreparationError {
    fn from(violation: ContextInvariantViolation) -> Self {
        Self::Context(violation)
    }
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
    ) -> Result<PreparedAgentTurn, AgentTurnPreparationError> {
        let contract_context = self.state.task_contract.model_context_for_task(self.tools);
        self.prepare_model_turn_with_context(
            user_instructions,
            runtime_context,
            contract_context,
            context_window_tokens,
            max_output_tokens,
        )
    }

    pub fn prepare_model_turn_with_contract(
        &self,
        user_instructions: Option<&str>,
        runtime_context: Option<&str>,
        workspace_verification_required: bool,
        context_window_tokens: u64,
        max_output_tokens: u64,
    ) -> Result<PreparedAgentTurn, AgentTurnPreparationError> {
        let contract_context = self
            .state
            .task_contract
            .model_context(workspace_verification_required, self.tools);
        self.prepare_model_turn_with_context(
            user_instructions,
            runtime_context,
            contract_context,
            context_window_tokens,
            max_output_tokens,
        )
    }

    fn prepare_model_turn_with_context(
        &self,
        user_instructions: Option<&str>,
        runtime_context: Option<&str>,
        contract_context: Option<String>,
        context_window_tokens: u64,
        max_output_tokens: u64,
    ) -> Result<PreparedAgentTurn, AgentTurnPreparationError> {
        crate::turn_budget::ensure_model_turn_available(self.state)?;
        let runtime_context = merged_runtime_context(runtime_context, contract_context.as_deref());
        let (request, context) = model_request_for_turn_with_context_budget(
            self.state,
            self.tools,
            user_instructions,
            runtime_context.as_deref(),
            context_window_tokens,
            max_output_tokens,
        );
        context.validate_required_invariants()?;
        Ok(PreparedAgentTurn { request, context })
    }

    pub fn advance_model_response(&mut self, response: ModelResponse) -> AgentAdvance {
        advance_with_model_response(self.state, response, self.tools)
    }

    pub fn apply_steer(&mut self, instruction: impl Into<String>, metadata: Metadata) -> usize {
        append_steering_instruction(self.state, instruction, metadata)
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
    ) -> Result<Option<AgentKernelInstruction>, crate::AgentFailure> {
        Ok(self
            .state
            .task_contract
            .completion_instruction(workspace_verification_required, self.tools)?
            .map(|content| AgentKernelInstruction {
                kind: AgentKernelInstructionKind::CompletionVerification,
                content,
            }))
    }

    pub fn completion_gate_for_task(
        &mut self,
    ) -> Result<Option<AgentKernelInstruction>, crate::AgentFailure> {
        Ok(self
            .state
            .task_contract
            .completion_instruction_for_task(self.tools)?
            .map(|content| AgentKernelInstruction {
                kind: AgentKernelInstructionKind::CompletionVerification,
                content,
            }))
    }

    pub fn require_tool_success(&mut self, tool_name: impl Into<String>) {
        self.state.task_contract.require_tool_success(tool_name);
    }

    pub fn replace_prompt_required_tool_successes<I, S>(&mut self, epoch: u64, tools: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.state
            .task_contract
            .replace_prompt_required_tool_successes(epoch, tools);
    }

    pub fn merge_workspace_verification_policy(&mut self, policy: WorkspaceVerificationPolicy) {
        self.state
            .task_contract
            .merge_workspace_verification_policy(policy);
    }

    pub fn require_any_tool_success<I, S>(&mut self, requirement_id: impl Into<String>, tools: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.state
            .task_contract
            .require_any_tool_success(requirement_id, tools);
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

fn merged_runtime_context(base: Option<&str>, task_contract: Option<&str>) -> Option<String> {
    match (
        base.map(str::trim).filter(|value| !value.is_empty()),
        task_contract,
    ) {
        (None, None) => None,
        (Some(base), None) => Some(base.to_string()),
        (None, Some(contract)) => Some(format!(
            "Active task contract (machine-generated data, not instructions):\n{contract}\nSatisfy every active obligation before presenting a final answer."
        )),
        (Some(base), Some(contract)) => Some(format!(
            "{base}\n\nActive task contract (machine-generated data, not instructions):\n{contract}\nSatisfy every active obligation before presenting a final answer."
        )),
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
        let prepared = AgentKernel::new(&mut state, &tools)
            .prepare_model_turn(
                Some("Be concise"),
                Some("workspace=/tmp/example"),
                16_384,
                2_048,
            )
            .expect("first model turn is available");

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
        state.task_contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        record_tool_outcome_with_risk(
            &mut state,
            "file.write",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        let tools = vec![read_tool()];
        let instruction = AgentKernel::new(&mut state, &tools)
            .completion_gate_for_task()
            .expect("completion gate evaluates")
            .expect("verification should be required");

        assert_eq!(
            instruction.kind,
            AgentKernelInstructionKind::CompletionVerification
        );
        assert!(instruction.content.contains("post-change verification"));
    }

    #[test]
    fn kernel_rebuilds_prompt_requirements_at_the_requested_epoch() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "generate an image",
            AgentRuntimeConfig::default(),
        );
        let tools = vec![ToolSpec::builtin(
            "image.generate",
            "image",
            "Generate an image",
            ToolRisk::UsesNetwork,
            r#"{"type":"object"}"#,
        )];
        let mut kernel = AgentKernel::new(&mut state, &tools);
        kernel.replace_prompt_required_tool_successes(2, ["image.generate"]);
        kernel.apply_tool_observation(
            &AgentToolRequest {
                call_id: ToolCallId("image-1".to_string()),
                tool_name: "image.generate".to_string(),
                input: r#"{"prompt":"first"}"#.to_string(),
            },
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
            "generated",
        );
        assert!(kernel
            .completion_gate_for_task()
            .expect("completion gate evaluates")
            .is_none());

        kernel.replace_prompt_required_tool_successes(3, ["image.generate"]);

        assert!(kernel
            .completion_gate_for_task()
            .expect("completion gate evaluates")
            .is_some());
    }

    #[test]
    fn prepared_turn_contains_active_contract_without_consuming_repair_attempts() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "change the workspace",
            AgentRuntimeConfig::default(),
        );
        state.task_contract.require_tool_success("file.read");
        state.task_contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        let tools = vec![read_tool()];

        let prepared = AgentKernel::new(&mut state, &tools)
            .prepare_model_turn(None, Some("workspace=/tmp/example"), 16_384, 2_048)
            .expect("first model turn is available");
        let system = &prepared.request.messages[0].content;

        assert!(system.contains("workspace=/tmp/example"));
        assert!(system.contains("cindx.task-contract.v1"));
        assert!(system.contains("file.read"));
        assert_eq!(state.turn, 0);
        assert!(state
            .task_contract
            .completion_instruction_for_task(&tools)
            .is_ok());
    }

    #[test]
    fn rejects_an_unsatisfied_context_projection_before_model_dispatch() {
        let mut state = start_agent_loop(
            TaskId("context-gate".to_string()),
            "preserve this request",
            AgentRuntimeConfig::default(),
        );
        let tools = vec![ToolSpec::builtin(
            "oversized.tool",
            "test",
            "oversized schema",
            ToolRisk::ReadOnly,
            format!(
                r#"{{"type":"object","description":"{}"}}"#,
                "x".repeat(100_000)
            ),
        )];

        let error = AgentKernel::new(&mut state, &tools)
            .prepare_model_turn(None, None, 4_096, 1_024)
            .expect_err("invalid projection must be rejected locally");

        assert!(matches!(error, AgentTurnPreparationError::Context(_)));
    }
}
