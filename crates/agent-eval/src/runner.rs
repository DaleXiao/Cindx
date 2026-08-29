//! Case runner: materializes the fixture into an isolated per-case workspace,
//! drives the portable `agent-runtime` loop with an injected provider, and
//! judges the result with pure postcondition checks.
//!
//! Loop driving choice: the runner uses the simplest viable surface —
//! `start_agent_loop` + `model_request_for_turn` + `advance_with_model_response`
//! with manual tool execution through the `tools` registry — instead of the
//! desktop composition root, which would drag in Tauri/desktop state.
//!
//! Permission policy: production grants execution through the desktop
//! permission path; the eval sandbox auto-admits only the case's declared
//! `allowed_tools` inside the isolated case workspace and denies everything
//! else, so runs stay deterministic and provider-free. Shell commands come
//! from the frozen suite and run with the tool's default argv; the isolation
//! boundary is the fresh per-case workspace (pinning the seatbelt sandbox
//! modes would make checks depend on the host OS sandbox).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use agent_core::{
    Message, MessageRole, Metadata, ModelRequest, ModelResponse, ModelToolCall, TaskId,
    ToolOutcomeStatus, ToolResult, ToolSpec,
};
use agent_runtime::{
    advance_with_model_response, append_observation, append_tool_observation,
    compose_base_agent_system_prompt, model_request_for_turn_with_system_prompt,
    observation_from_agent_tool_result, record_tool_outcome, start_agent_loop,
    tool_invocation_from_request, AgentAdvance, AgentRuntimeConfig,
    AgentToolRequest,
};
use tools::ToolRegistry;

use crate::case::EvalCase;
use crate::postcondition::{check_postconditions, CheckResult};

/// Deterministic eval-side error: the crate never performs network I/O, so
/// every failure is structural (script, fixture, budget, loop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError {
    pub code: String,
    pub message: String,
}

impl EvalError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EvalError {}

/// Injected model side. Implementations must be deterministic for replayable
/// suites; the runner never constructs a network transport itself.
pub trait EvalModelProvider {
    fn name(&self) -> &str;

    fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, EvalError>;
}

/// One scripted tool call emitted by [`ScriptedProvider`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScriptedToolCall {
    pub name: String,
    pub arguments_json: String,
}

/// One scripted provider step: either the final answer or a tool-call batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptedStep {
    Final(String),
    ToolCalls(Vec<ScriptedToolCall>),
}

/// Replayable provider that returns a fixed queue of steps and fails closed
/// when the queue is exhausted.
pub struct ScriptedProvider {
    steps: VecDeque<ScriptedStep>,
    next_call: usize,
}

impl ScriptedProvider {
    pub fn new(steps: Vec<ScriptedStep>) -> Self {
        Self {
            steps: steps.into(),
            next_call: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        self.steps.len()
    }
}

impl EvalModelProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn complete(&mut self, _request: &ModelRequest) -> Result<ModelResponse, EvalError> {
        let step = self.steps.pop_front().ok_or_else(|| {
            EvalError::new(
                "script_exhausted",
                "scripted provider has no remaining steps",
            )
        })?;
        match step {
            ScriptedStep::Final(text) => Ok(ModelResponse {
                message: Message {
                    role: MessageRole::Assistant,
                    content: text,
                    metadata: Metadata::new(),
                },
                raw_tool_calls_json: None,
                tool_calls: Vec::new(),
                metadata: [("finish_reason".to_string(), "stop".to_string())]
                    .into_iter()
                    .collect(),
            }),
            ScriptedStep::ToolCalls(calls) => {
                let mut tool_calls = Vec::with_capacity(calls.len());
                for call in calls {
                    self.next_call += 1;
                    tool_calls.push(ModelToolCall {
                        id: format!("eval-call-{}", self.next_call),
                        name: call.name,
                        arguments_json: call.arguments_json,
                    });
                }
                Ok(ModelResponse {
                    message: Message {
                        role: MessageRole::Assistant,
                        content: String::new(),
                        metadata: Metadata::new(),
                    },
                    raw_tool_calls_json: None,
                    tool_calls,
                    metadata: [("finish_reason".to_string(), "tool_calls".to_string())]
                        .into_iter()
                        .collect(),
                })
            }
        }
    }
}

/// Outcome of one case run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CaseReport {
    pub id: String,
    pub passed: bool,
    pub checks: Vec<CheckResult>,
    pub tool_calls: usize,
    pub turns: usize,
    pub error: Option<String>,
}

struct PreparedCase {
    registry: ToolRegistry,
    specs: Vec<ToolSpec>,
}

/// Run one case to completion (or budget exhaustion) and judge it.
///
/// The case gets a fresh workspace at `workspace_root/<case.id>` (any stale
/// directory under that eval-owned name is reset first), so cases never share
/// state.
pub fn run_case<P: EvalModelProvider>(
    case: &EvalCase,
    provider: &mut P,
    workspace_root: &Path,
) -> CaseReport {
    let case_root = workspace_root.join(&case.id);
    let mut final_answer = String::new();
    let mut tool_call_count = 0usize;
    let mut turns = 0usize;
    let error: Option<EvalError>;

    match prepare_case_workspace(case, &case_root) {
        Err(preparation_error) => error = Some(preparation_error),
        Ok(prepared) => {
            let (answer, calls, completed_turns, loop_error) = run_loop(case, &prepared, provider);
            final_answer = answer;
            tool_call_count = calls;
            turns = completed_turns;
            error = loop_error;
        }
    }

    let checks = check_postconditions(&case_root, &final_answer, &case.postconditions);
    let passed = error.is_none() && checks.iter().all(|check| check.passed);
    CaseReport {
        id: case.id.clone(),
        passed,
        checks,
        tool_calls: tool_call_count,
        turns,
        error: error.map(|eval_error| eval_error.to_string()),
    }
}

/// Reset and materialize the case workspace, then build the admitted tool
/// registry and the exposed specs (only `allowed_tools`).
fn prepare_case_workspace(case: &EvalCase, case_root: &Path) -> Result<PreparedCase, EvalError> {
    if case_root.exists() {
        std::fs::remove_dir_all(case_root).map_err(|error| {
            EvalError::new(
                "workspace_reset_failed",
                format!("cannot reset {}: {error}", case_root.display()),
            )
        })?;
    }
    std::fs::create_dir_all(case_root).map_err(|error| {
        EvalError::new(
            "workspace_create_failed",
            format!("cannot create {}: {error}", case_root.display()),
        )
    })?;

    for fixture in &case.fixture {
        let target = fixture_target(case_root, &fixture.path)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                EvalError::new(
                    "fixture_materialize_failed",
                    format!("cannot create parent of {}: {error}", fixture.path),
                )
            })?;
        }
        std::fs::write(&target, &fixture.content).map_err(|error| {
            EvalError::new(
                "fixture_materialize_failed",
                format!("cannot write fixture {}: {error}", fixture.path),
            )
        })?;
    }

    let registry = ToolRegistry::with_workspace_tools(case_root);
    let mut specs = Vec::new();
    for name in &case.allowed_tools {
        match registry.get(name) {
            Some(tool) => specs.push(tool.spec()),
            None => {
                return Err(EvalError::new(
                    "unknown_tool",
                    format!("allowed tool is not registered: {name}"),
                ))
            }
        }
    }
    specs.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(PreparedCase { registry, specs })
}

/// Resolve a fixture path inside the case root; reject absolute paths and
/// parent-dir escapes fail-closed.
fn fixture_target(case_root: &Path, fixture_path: &str) -> Result<PathBuf, EvalError> {
    let candidate = Path::new(fixture_path);
    if candidate.is_absolute() {
        return Err(EvalError::new(
            "fixture_path_escape",
            format!("absolute fixture path is not allowed: {fixture_path}"),
        ));
    }
    for component in candidate.components() {
        if matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        ) {
            return Err(EvalError::new(
                "fixture_path_escape",
                format!("fixture path escapes the case workspace: {fixture_path}"),
            ));
        }
    }
    Ok(case_root.join(candidate))
}

/// Drive the runtime loop until completion, budget exhaustion, or failure.
/// Returns `(final_answer, tool_call_count, turns, error)`.
fn run_loop<P: EvalModelProvider>(
    case: &EvalCase,
    prepared: &PreparedCase,
    provider: &mut P,
) -> (String, usize, usize, Option<EvalError>) {
    let mut state = start_agent_loop(
        TaskId(format!("eval:{}", case.id)),
        &case.prompt,
        AgentRuntimeConfig {
            max_turns: case.budget.max_turns,
        },
    );
    let mut tool_call_count = 0usize;
    // Measure the product agent, not a bare loop: inject the same base system
    // prompt the desktop composes (core contract + no user override).
    let system_prompt = compose_base_agent_system_prompt(None);

    loop {
        let request =
            model_request_for_turn_with_system_prompt(&state, &prepared.specs, Some(&system_prompt));
        let response = match provider.complete(&request) {
            Ok(response) => response,
            Err(provider_error) => {
                return (
                    String::new(),
                    tool_call_count,
                    state.turn,
                    Some(provider_error),
                )
            }
        };
        match advance_with_model_response(&mut state, response, &prepared.specs) {
            AgentAdvance::Completed { answer } => {
                return (answer, tool_call_count, state.turn, None)
            }
            AgentAdvance::ToolCalls { calls } => {
                let requested = tool_call_count + calls.len();
                if requested > case.budget.max_tool_calls {
                    return (
                        String::new(),
                        tool_call_count,
                        state.turn,
                        Some(EvalError::new(
                            "tool_call_budget_exhausted",
                            format!(
                                "case requested {requested} tool calls but the budget allows {}",
                                case.budget.max_tool_calls
                            ),
                        )),
                    );
                }
                for call in &calls {
                    let result = execute_tool_call(&state.task_id, case, prepared, call);
                    record_tool_outcome(&mut state, &call.tool_name, &call.input, &result.status);
                    let observation = observation_from_agent_tool_result(&call.tool_name, &result);
                    append_tool_observation(&mut state, call.call_id.clone(), &observation);
                    tool_call_count += 1;
                }
            }
            AgentAdvance::TurnBudgetExhausted(exhausted) => {
                return (
                    exhausted.partial_answer.unwrap_or_default(),
                    tool_call_count,
                    state.turn,
                    Some(EvalError::new(
                        "turn_budget_exhausted",
                        format!(
                            "loop stopped after {} of {} turns",
                            exhausted.completed_turns, exhausted.max_turns
                        ),
                    )),
                )
            }
            AgentAdvance::Retry { instruction } => append_observation(&mut state, &instruction),
            AgentAdvance::Failed { failure } => {
                return (
                    String::new(),
                    tool_call_count,
                    state.turn,
                    Some(EvalError::new(failure.code, failure.message)),
                )
            }
        }
    }
}

/// Execute one admitted tool call inside the case workspace. Denied or
/// unknown tools and tool errors become failed observations the provider can
/// recover from, mirroring how the loop consumes tool failures.
fn execute_tool_call(
    task_id: &TaskId,
    case: &EvalCase,
    prepared: &PreparedCase,
    call: &AgentToolRequest,
) -> ToolResult {
    let admitted = case
        .allowed_tools
        .iter()
        .any(|name| name == &call.tool_name);
    let tool = admitted
        .then(|| prepared.registry.get(&call.tool_name))
        .flatten();
    let Some(tool) = tool else {
        return ToolResult::text(
            call.call_id.clone(),
            ToolOutcomeStatus::Denied,
            format!("tool {} is not admitted by this case", call.tool_name),
            Metadata::new(),
        );
    };

    let invocation = tool_invocation_from_request(task_id, call);
    match tool.execute(invocation) {
        Ok(result) => result,
        Err(tool_error) => ToolResult::text(
            call.call_id.clone(),
            ToolOutcomeStatus::Failed,
            format!("{}: {}", tool_error.code, tool_error.message),
            Metadata::new(),
        ),
    }
}
