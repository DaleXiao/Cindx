use crate::{
    sanitize_assistant_content, start_agent_loop, tool_invocation_from_request, AgentAdvance,
    AgentFailure, AgentKernel, AgentLoopState, AgentRuntimeConfig, AgentToolRequest,
    AgentTurnBudgetExhausted, AgentTurnPreparationError, ContextGovernorReport, PreparedAgentTurn,
    WorkerTurnPhase, WorkerTurnPolicy, MAX_IDENTICAL_TOOL_FAILURES,
};
use agent_core::{Metadata, TaskId, ToolInvocation, ToolOutcomeStatus, ToolRisk, ToolSpec};
use model_provider::ModelResponse;

const WORKER_TURN_BUDGET_EXHAUSTED: &str = "worker_turn_budget_exhausted";
const WORKER_FINALIZATION_TOOL_CALL: &str = "worker_tool_calls_after_evidence_phase";
const WORKER_TASK_CONTRACT_UNSATISFIED: &str = "worker_task_contract_unsatisfied";
const SUBSTANTIVE_EVIDENCE_REQUIREMENT: &str = "substantive_read_only_evidence";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerFailure {
    pub failure: AgentFailure,
    pub partial_content: Option<String>,
}

impl WorkerFailure {
    fn from_turn_budget(exhausted: AgentTurnBudgetExhausted) -> Self {
        Self {
            failure: AgentFailure::budget(
                WORKER_TURN_BUDGET_EXHAUSTED,
                format!(
                    "worker_turn_budget_exhausted: completed {} turns with a {}-turn budget",
                    exhausted.completed_turns, exhausted.max_turns
                ),
            ),
            partial_content: exhausted.partial_answer,
        }
    }

    fn from_turn_preparation(error: AgentTurnPreparationError) -> Self {
        match error {
            AgentTurnPreparationError::Budget(exhausted) => Self::from_turn_budget(exhausted),
            AgentTurnPreparationError::Context(violation) => {
                Self::from_failure(AgentFailure::contract(
                    "context_projection_invariant_failed",
                    violation.to_string(),
                ))
            }
        }
    }

    pub fn from_failure(failure: AgentFailure) -> Self {
        Self {
            failure,
            partial_content: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedWorkerTurn {
    pub turn: PreparedAgentTurn,
    pub phase: WorkerTurnPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerAdvance {
    Completed { answer: String },
    ToolCalls { calls: Vec<AgentToolRequest> },
    Retry,
    Failed(WorkerFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerToolDenialKind {
    BudgetExhausted,
    RepeatedFailure,
    NotExposed,
    NotReadOnly,
}

impl WorkerToolDenialKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::BudgetExhausted => "worker_tool_budget_exhausted",
            Self::RepeatedFailure => "worker_repeated_tool_failure",
            Self::NotExposed => "worker_tool_not_exposed",
            Self::NotReadOnly => "worker_tool_not_read_only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerToolAdmission {
    Allowed,
    Denied {
        kind: WorkerToolDenialKind,
        reason: &'static str,
    },
}

/// Pure state machine for bounded, read-only collaboration and evaluation workers.
///
/// Provider calls, event persistence, cancellation and concrete tool execution stay
/// in adapters. This type is the single owner of worker turn semantics, tool budgets,
/// finalization behavior and usage aggregation.
pub struct IsolatedWorkerRuntime {
    state: AgentLoopState,
    tools: Vec<ToolSpec>,
    turn_policy: WorkerTurnPolicy,
    current_phase: WorkerTurnPhase,
    max_tool_calls: usize,
    tool_call_count: usize,
    evidence_repair_pending: bool,
    evidence_repair_used: bool,
    prepared_message_count: usize,
    usage: Metadata,
}

impl IsolatedWorkerRuntime {
    pub fn new(
        task_id: TaskId,
        prompt: impl Into<String>,
        tools: Vec<ToolSpec>,
        evidence_turns: usize,
        max_tool_calls: usize,
    ) -> Self {
        let turn_policy = WorkerTurnPolicy::isolated_evidence(evidence_turns, !tools.is_empty());
        Self {
            state: start_agent_loop(
                task_id,
                prompt,
                AgentRuntimeConfig {
                    max_turns: turn_policy.runtime_turn_limit(),
                },
            ),
            tools,
            turn_policy,
            current_phase: WorkerTurnPhase::Evidence,
            max_tool_calls,
            tool_call_count: 0,
            evidence_repair_pending: false,
            evidence_repair_used: false,
            prepared_message_count: 0,
            usage: Metadata::new(),
        }
    }

    pub fn task_id(&self) -> &TaskId {
        &self.state.task_id
    }

    pub fn turn(&self) -> usize {
        self.state.turn
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    pub fn tool_call_count(&self) -> usize {
        self.tool_call_count
    }

    /// Requires at least one content-bearing read-only observation before this
    /// worker may complete. Discovery surfaces remain available, but directory,
    /// catalog and tab enumeration alone cannot satisfy the contract.
    pub fn require_substantive_evidence(&mut self) -> usize {
        let alternatives = self
            .tools
            .iter()
            .filter(|tool| substantive_evidence_tool(tool))
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        if !alternatives.is_empty() {
            AgentKernel::new(&mut self.state, &self.tools)
                .require_any_tool_success(SUBSTANTIVE_EVIDENCE_REQUIREMENT, alternatives.clone());
        }
        alternatives.len()
    }

    pub fn prepare_model_turn(
        &mut self,
        user_instructions: Option<&str>,
        runtime_context: Option<&str>,
        context_window_tokens: u64,
        max_output_tokens: u64,
    ) -> Result<PreparedWorkerTurn, WorkerFailure> {
        let policy_phase = self.turn_policy.phase_for_turn(self.state.turn);
        self.current_phase = if self.evidence_repair_pending {
            WorkerTurnPhase::Evidence
        } else if self.evidence_repair_used {
            WorkerTurnPhase::Finalization
        } else if policy_phase == WorkerTurnPhase::Finalization
            && self.turn_policy.supports_evidence_repair()
        {
            let instruction = AgentKernel::new(&mut self.state, &self.tools)
                .completion_gate_for_task()
                .map_err(WorkerFailure::from_failure)?;
            if let Some(instruction) = instruction {
                AgentKernel::new(&mut self.state, &self.tools).apply_instruction(&instruction);
                self.evidence_repair_pending = true;
                self.evidence_repair_used = true;
                WorkerTurnPhase::Evidence
            } else {
                WorkerTurnPhase::Finalization
            }
        } else {
            policy_phase
        };
        if self.current_phase == WorkerTurnPhase::Finalization {
            self.turn_policy.prepare_finalization(&mut self.state);
        }
        self.prepared_message_count = self.state.messages.len();
        let phase = self.current_phase;
        let (state, tools) = (&mut self.state, &self.tools);
        let active_tools = if phase == WorkerTurnPhase::Finalization {
            &[]
        } else {
            tools.as_slice()
        };
        let turn = AgentKernel::new(state, active_tools)
            .prepare_model_turn(
                user_instructions,
                runtime_context,
                context_window_tokens,
                max_output_tokens,
            )
            .map_err(WorkerFailure::from_turn_preparation)?;
        self.record_context(&turn.context);
        Ok(PreparedWorkerTurn {
            turn,
            phase: self.current_phase,
        })
    }

    pub fn advance_model_response(&mut self, response: ModelResponse) -> WorkerAdvance {
        self.record_response_usage(&response.metadata);
        let finalization_content = (self.current_phase == WorkerTurnPhase::Finalization)
            .then(|| sanitize_assistant_content(&response.message.content))
            .filter(|content| !content.is_empty());
        let phase = self.current_phase;
        let (state, tools) = (&mut self.state, &self.tools);
        let active_tools = if phase == WorkerTurnPhase::Finalization {
            &[]
        } else {
            tools.as_slice()
        };
        match AgentKernel::new(state, active_tools).advance_model_response(response) {
            AgentAdvance::Completed { answer } => self.complete_or_repair(answer),
            AgentAdvance::ToolCalls { calls: _ }
                if self.current_phase == WorkerTurnPhase::Finalization =>
            {
                if let Some(answer) = finalization_content {
                    self.complete_or_repair(answer)
                } else {
                    WorkerAdvance::Failed(WorkerFailure::from_failure(
                        AgentFailure::model_output(
                            WORKER_FINALIZATION_TOOL_CALL,
                            "worker requested another tool after its evidence phase; final answer was empty",
                        ),
                    ))
                }
            }
            AgentAdvance::ToolCalls { calls } => {
                if self.evidence_repair_pending {
                    self.evidence_repair_pending = false;
                }
                WorkerAdvance::ToolCalls { calls }
            }
            AgentAdvance::TurnBudgetExhausted(exhausted) => {
                WorkerAdvance::Failed(WorkerFailure::from_turn_budget(exhausted))
            }
            AgentAdvance::Failed { failure } => {
                WorkerAdvance::Failed(WorkerFailure::from_failure(failure))
            }
            AgentAdvance::Retry { instruction } => {
                if self.evidence_repair_pending {
                    self.evidence_repair_pending = false;
                }
                let phase = self.current_phase;
                let (state, tools) = (&mut self.state, &self.tools);
                let active_tools = if phase == WorkerTurnPhase::Finalization {
                    &[]
                } else {
                    tools.as_slice()
                };
                AgentKernel::new(state, active_tools).apply_model_response_retry(instruction);
                WorkerAdvance::Retry
            }
        }
    }

    pub fn admit_tool_call(&mut self, request: &AgentToolRequest) -> WorkerToolAdmission {
        if self.tool_call_count >= self.max_tool_calls {
            return WorkerToolAdmission::Denied {
                kind: WorkerToolDenialKind::BudgetExhausted,
                reason: "This worker exhausted its evidence-tool budget. Stop searching and return the best concise result from existing evidence.",
            };
        }
        if AgentKernel::new(&mut self.state, &self.tools).repeated_tool_failure_count(request)
            >= MAX_IDENTICAL_TOOL_FAILURES
        {
            return WorkerToolAdmission::Denied {
                kind: WorkerToolDenialKind::RepeatedFailure,
                reason: "Cindx blocked this identical worker tool call after repeated failures. Change the arguments or use a different approach.",
            };
        }
        let Some(tool) = self
            .tools
            .iter()
            .find(|tool| tool.name == request.tool_name)
        else {
            return WorkerToolAdmission::Denied {
                kind: WorkerToolDenialKind::NotExposed,
                reason: "This tool is not exposed to the isolated worker.",
            };
        };
        if tool.risk != ToolRisk::ReadOnly {
            return WorkerToolAdmission::Denied {
                kind: WorkerToolDenialKind::NotReadOnly,
                reason: "Isolated workers may execute only read-only evidence tools.",
            };
        }
        self.tool_call_count = self.tool_call_count.saturating_add(1);
        WorkerToolAdmission::Allowed
    }

    pub fn tool_invocation(&self, request: &AgentToolRequest) -> ToolInvocation {
        tool_invocation_from_request(&self.state.task_id, request)
    }

    pub fn apply_tool_observation(
        &mut self,
        request: &AgentToolRequest,
        status: &ToolOutcomeStatus,
        observation: &str,
    ) {
        let risk = self
            .tools
            .iter()
            .find(|tool| tool.name == request.tool_name)
            .map(|tool| &tool.risk);
        AgentKernel::new(&mut self.state, &self.tools).apply_tool_observation(
            request,
            status,
            risk,
            observation,
        );
    }

    pub fn insert_usage(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.usage.insert(key.into(), value.into());
    }

    pub fn completion_usage(&self, runtime_label: &str) -> Metadata {
        let mut usage = self.usage.clone();
        usage.insert("worker_turns".to_string(), self.turn().to_string());
        usage.insert(
            "worker_tool_calls".to_string(),
            self.tool_call_count().to_string(),
        );
        usage.insert(
            "worker_tool_count".to_string(),
            self.tool_count().to_string(),
        );
        usage.insert("worker_runtime".to_string(), runtime_label.to_string());
        usage.insert(
            "worker_contract_repair_used".to_string(),
            self.evidence_repair_used.to_string(),
        );
        usage
    }

    fn complete_or_repair(&mut self, answer: String) -> WorkerAdvance {
        let gate = AgentKernel::new(&mut self.state, &self.tools).completion_gate_for_task();
        match gate {
            Ok(None) => WorkerAdvance::Completed { answer },
            Ok(Some(instruction))
                if self.turn_policy.supports_evidence_repair() && !self.evidence_repair_used =>
            {
                self.state
                    .messages
                    .truncate(self.prepared_message_count.min(self.state.messages.len()));
                AgentKernel::new(&mut self.state, &self.tools).apply_instruction(&instruction);
                self.evidence_repair_pending = true;
                self.evidence_repair_used = true;
                WorkerAdvance::Retry
            }
            Ok(Some(instruction)) => {
                self.state
                    .messages
                    .truncate(self.prepared_message_count.min(self.state.messages.len()));
                WorkerAdvance::Failed(WorkerFailure::from_failure(AgentFailure::contract(
                    WORKER_TASK_CONTRACT_UNSATISFIED,
                    instruction.content,
                )))
            }
            Err(failure) => {
                self.state
                    .messages
                    .truncate(self.prepared_message_count.min(self.state.messages.len()));
                WorkerAdvance::Failed(WorkerFailure::from_failure(failure))
            }
        }
    }

    fn record_context(&mut self, report: &ContextGovernorReport) {
        self.usage.insert(
            "context_governor_applied".to_string(),
            report.applied.to_string(),
        );
        self.usage.insert(
            "context_projected_tokens".to_string(),
            report.estimated_projected_tokens.to_string(),
        );
    }

    fn record_response_usage(&mut self, metadata: &Metadata) {
        for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
            let previous = self
                .usage
                .get(key)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default();
            let additional = metadata
                .get(key)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default();
            self.usage.insert(
                key.to_string(),
                previous.saturating_add(additional).to_string(),
            );
        }

        let previous_source = self.usage.get("usage_source").map(String::as_str);
        let additional_source = metadata.get("usage_source").map(String::as_str);
        let combined_source = if [previous_source, additional_source]
            .into_iter()
            .flatten()
            .any(|source| source == "estimated")
        {
            "estimated"
        } else if [previous_source, additional_source]
            .into_iter()
            .flatten()
            .any(|source| source == "provider_partial")
        {
            "provider_partial"
        } else {
            "provider"
        };
        self.usage
            .insert("usage_source".to_string(), combined_source.to_string());
        self.usage.insert(
            "usage_estimated".to_string(),
            (combined_source != "provider").to_string(),
        );
    }
}

fn substantive_evidence_tool(tool: &ToolSpec) -> bool {
    tool.risk == ToolRisk::ReadOnly
        && !matches!(
            tool.name.as_str(),
            "file.list" | "browser.tabs" | "tool.search" | "tool.inspect"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Message, MessageRole, ToolCallId};
    use model_provider::{ModelResponseTermination, ModelToolCall};

    fn read_tool() -> ToolSpec {
        ToolSpec::builtin(
            "file.read",
            "file",
            "Read a file",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )
    }

    fn list_tool() -> ToolSpec {
        ToolSpec::builtin(
            "file.list",
            "file",
            "List files",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )
    }

    fn read_many_tool() -> ToolSpec {
        ToolSpec::builtin(
            "file.read_many",
            "file",
            "Read multiple files",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )
    }

    fn response(content: &str, tool_calls: Vec<ModelToolCall>) -> ModelResponse {
        ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: content.to_string(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls,
            metadata: [
                ("prompt_tokens".to_string(), "10".to_string()),
                ("total_tokens".to_string(), "15".to_string()),
                ("usage_source".to_string(), "provider".to_string()),
                (
                    "termination".to_string(),
                    format!("{:?}", ModelResponseTermination::Complete),
                ),
            ]
            .into_iter()
            .collect(),
        }
    }

    fn tool_call(id: &str) -> ModelToolCall {
        ModelToolCall {
            id: id.to_string(),
            name: "file.read".to_string(),
            arguments_json: r#"{"path":"README.md"}"#.to_string(),
        }
    }

    fn named_tool_call(id: &str, name: &str, arguments_json: &str) -> ModelToolCall {
        ModelToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json: arguments_json.to_string(),
        }
    }

    #[test]
    fn finalization_tool_call_uses_content_instead_of_leaking_another_call() {
        let mut worker = IsolatedWorkerRuntime::new(
            TaskId("worker".to_string()),
            "inspect",
            vec![read_tool()],
            1,
            2,
        );
        worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("evidence turn");
        let calls = match worker.advance_model_response(response("", vec![tool_call("call-1")])) {
            WorkerAdvance::ToolCalls { calls } => calls,
            other => panic!("expected tool calls, got {other:?}"),
        };
        assert_eq!(
            worker.admit_tool_call(&calls[0]),
            WorkerToolAdmission::Allowed
        );
        worker.apply_tool_observation(&calls[0], &ToolOutcomeStatus::Succeeded, "ok");

        let prepared = worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("finalization turn");
        assert_eq!(prepared.phase, WorkerTurnPhase::Finalization);
        assert!(prepared.turn.request.tools.is_empty());
        assert_eq!(
            worker.advance_model_response(response("grounded answer", vec![tool_call("call-2")])),
            WorkerAdvance::Completed {
                answer: "grounded answer".to_string()
            }
        );
    }

    #[test]
    fn finalization_without_content_has_one_shared_failure_code() {
        let mut worker = IsolatedWorkerRuntime::new(
            TaskId("worker".to_string()),
            "inspect",
            vec![read_tool()],
            1,
            2,
        );
        worker.state.turn = 1;
        worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("finalization turn");
        let WorkerAdvance::Failed(failed) =
            worker.advance_model_response(response("", vec![tool_call("call-2")]))
        else {
            panic!("expected failure");
        };
        assert_eq!(failed.failure.code, WORKER_FINALIZATION_TOOL_CALL);
    }

    #[test]
    fn tool_admission_enforces_budget_and_repeated_failures() {
        let mut worker = IsolatedWorkerRuntime::new(
            TaskId("worker".to_string()),
            "inspect",
            vec![read_tool()],
            2,
            1,
        );
        let request = AgentToolRequest {
            call_id: ToolCallId("call-1".to_string()),
            tool_name: "file.read".to_string(),
            input: r#"{"path":"README.md"}"#.to_string(),
        };
        assert_eq!(
            worker.admit_tool_call(&request),
            WorkerToolAdmission::Allowed
        );
        assert!(matches!(
            worker.admit_tool_call(&request),
            WorkerToolAdmission::Denied {
                kind: WorkerToolDenialKind::BudgetExhausted,
                ..
            }
        ));
        assert_eq!(worker.tool_call_count(), 1);
    }

    #[test]
    fn rejected_tool_calls_do_not_consume_worker_budget() {
        let mut worker = IsolatedWorkerRuntime::new(
            TaskId("worker".to_string()),
            "inspect",
            vec![read_tool()],
            2,
            1,
        );
        let rejected = AgentToolRequest {
            call_id: ToolCallId("call-invalid".to_string()),
            tool_name: "shell.run".to_string(),
            input: r#"{"command":"true"}"#.to_string(),
        };
        assert!(matches!(
            worker.admit_tool_call(&rejected),
            WorkerToolAdmission::Denied {
                kind: WorkerToolDenialKind::NotExposed,
                ..
            }
        ));
        assert_eq!(worker.tool_call_count(), 0);

        let valid = AgentToolRequest {
            call_id: ToolCallId("call-valid".to_string()),
            tool_name: "file.read".to_string(),
            input: r#"{"path":"README.md"}"#.to_string(),
        };
        assert_eq!(worker.admit_tool_call(&valid), WorkerToolAdmission::Allowed);
        assert_eq!(worker.tool_call_count(), 1);
    }

    #[test]
    fn usage_sources_are_preserved_instead_of_parsed_as_numbers() {
        let mut worker =
            IsolatedWorkerRuntime::new(TaskId("worker".to_string()), "answer", Vec::new(), 2, 0);
        worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("first turn");
        assert!(matches!(
            worker.advance_model_response(response("one", Vec::new())),
            WorkerAdvance::Completed { .. }
        ));
        let usage = worker.completion_usage("test");
        assert_eq!(usage["prompt_tokens"], "10");
        assert_eq!(usage["total_tokens"], "15");
        assert_eq!(usage["usage_source"], "provider");
        assert_eq!(usage["usage_estimated"], "false");
    }

    #[test]
    fn no_tool_worker_recovers_once_from_an_empty_provider_response() {
        let mut worker =
            IsolatedWorkerRuntime::new(TaskId("worker".to_string()), "answer", Vec::new(), 1, 0);
        let first = worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("first evidence turn");
        assert_eq!(first.phase, WorkerTurnPhase::Evidence);
        assert_eq!(
            worker.advance_model_response(response("", Vec::new())),
            WorkerAdvance::Retry
        );

        let recovery = worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("protected recovery turn");
        assert_eq!(recovery.phase, WorkerTurnPhase::Finalization);
        assert_eq!(
            worker.advance_model_response(response("recovered answer", Vec::new())),
            WorkerAdvance::Completed {
                answer: "recovered answer".to_string()
            }
        );
    }

    #[test]
    fn discovery_only_evidence_gets_one_tool_capable_contract_repair_turn() {
        let mut worker = IsolatedWorkerRuntime::new(
            TaskId("worker".to_string()),
            "identify exact values from workspace files",
            vec![list_tool(), read_many_tool()],
            1,
            3,
        );
        assert_eq!(worker.require_substantive_evidence(), 1);

        let first = worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("discovery turn");
        assert_eq!(first.phase, WorkerTurnPhase::Evidence);
        let calls = match worker.advance_model_response(response(
            "",
            vec![named_tool_call("list", "file.list", r#"{"path":""}"#)],
        )) {
            WorkerAdvance::ToolCalls { calls } => calls,
            other => panic!("expected discovery tool call, got {other:?}"),
        };
        assert_eq!(
            worker.admit_tool_call(&calls[0]),
            WorkerToolAdmission::Allowed
        );
        worker.apply_tool_observation(&calls[0], &ToolOutcomeStatus::Succeeded, "file\t10\ta.md");

        let repair = worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("contract repair turn");
        assert_eq!(repair.phase, WorkerTurnPhase::Evidence);
        assert!(!repair.turn.request.tools.is_empty());
        assert!(repair
            .turn
            .request
            .messages
            .iter()
            .any(|message| message.content.contains(SUBSTANTIVE_EVIDENCE_REQUIREMENT)));

        let calls = match worker.advance_model_response(response(
            "",
            vec![named_tool_call(
                "read-many",
                "file.read_many",
                r#"{"paths":["a.md"]}"#,
            )],
        )) {
            WorkerAdvance::ToolCalls { calls } => calls,
            other => panic!("expected content tool call, got {other:?}"),
        };
        assert_eq!(
            worker.admit_tool_call(&calls[0]),
            WorkerToolAdmission::Allowed
        );
        worker.apply_tool_observation(
            &calls[0],
            &ToolOutcomeStatus::Succeeded,
            "a.md\nexact value",
        );

        let finalization = worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("finalization turn");
        assert_eq!(finalization.phase, WorkerTurnPhase::Finalization);
        assert!(finalization.turn.request.tools.is_empty());
        assert_eq!(
            worker.advance_model_response(response("exact value", Vec::new())),
            WorkerAdvance::Completed {
                answer: "exact value".to_string()
            }
        );
    }

    #[test]
    fn repair_that_still_has_no_substantive_evidence_fails_closed() {
        let mut worker = IsolatedWorkerRuntime::new(
            TaskId("worker".to_string()),
            "identify exact values from workspace files",
            vec![list_tool(), read_tool()],
            1,
            2,
        );
        worker.require_substantive_evidence();
        worker.state.turn = 1;

        let repair = worker
            .prepare_model_turn(None, None, 16_384, 1_024)
            .expect("contract repair turn");
        assert_eq!(repair.phase, WorkerTurnPhase::Evidence);
        let WorkerAdvance::Failed(failed) =
            worker.advance_model_response(response("insufficient evidence", Vec::new()))
        else {
            panic!("expected a closed contract failure");
        };
        assert_eq!(failed.failure.code, WORKER_TASK_CONTRACT_UNSATISFIED);
    }
}
