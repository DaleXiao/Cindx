use agent_core::{Message, MessageRole, Metadata, ModelRole, TaskId, ToolCallId, ToolInvocation, ToolOutcomeStatus, ToolSpec};
use model_provider::{tool_function_name, ModelCallMode, ModelRequest, ModelResponse};
use std::collections::BTreeMap;

pub const DEFAULT_MAX_AGENT_TURNS: usize = 24;
pub const MAX_IDENTICAL_TOOL_FAILURES: usize = 2;
pub const DEFAULT_AGENT_SYSTEM_PROMPT: &str = "You are Cindx, a desktop-first assistant. Work carefully, be direct, and ask for clarification when the task is ambiguous.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntimeConfig {
    pub max_turns: usize,
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_AGENT_TURNS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLoopState {
    pub task_id: TaskId,
    pub user_prompt: String,
    pub messages: Vec<Message>,
    pub turn: usize,
    pub max_turns: usize,
    pub failed_tool_signatures: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentToolRequest {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAdvance {
    Completed { answer: String },
    ToolCalls { calls: Vec<AgentToolRequest> },
    Failed { message: String },
}

pub fn start_agent_loop(
    task_id: TaskId,
    user_prompt: impl Into<String>,
    config: AgentRuntimeConfig,
) -> AgentLoopState {
    let user_prompt = user_prompt.into();
    AgentLoopState {
        task_id,
        messages: vec![Message {
            role: MessageRole::User,
            content: user_prompt.clone(),
            metadata: Metadata::new(),
        }],
        user_prompt,
        turn: 0,
        max_turns: config.max_turns.max(1),
        failed_tool_signatures: BTreeMap::new(),
    }
}

pub fn start_agent_loop_with_history(
    task_id: TaskId,
    user_prompt: impl Into<String>,
    mut history: Vec<Message>,
    config: AgentRuntimeConfig,
) -> AgentLoopState {
    let user_prompt = user_prompt.into();
    history.push(Message {
        role: MessageRole::User,
        content: user_prompt.clone(),
        metadata: Metadata::new(),
    });
    AgentLoopState {
        task_id,
        user_prompt,
        messages: history,
        turn: 0,
        max_turns: config.max_turns.max(1),
        failed_tool_signatures: BTreeMap::new(),
    }
}

pub fn resume_agent_loop(
    task_id: TaskId,
    user_prompt: impl Into<String>,
    observations: &[String],
    config: AgentRuntimeConfig,
) -> AgentLoopState {
    let mut state = start_agent_loop(task_id, user_prompt, config);
    for observation in observations {
        append_observation(&mut state, observation);
    }
    state
}

pub fn resume_agent_loop_from_messages(
    task_id: TaskId,
    user_prompt: impl Into<String>,
    messages: Vec<Message>,
    config: AgentRuntimeConfig,
) -> AgentLoopState {
    let turn = messages
        .iter()
        .filter(|message| matches!(message.role, MessageRole::Assistant))
        .count();
    AgentLoopState {
        task_id,
        user_prompt: user_prompt.into(),
        messages,
        turn,
        max_turns: config.max_turns.max(1),
        failed_tool_signatures: BTreeMap::new(),
    }
}

pub fn model_request_for_turn(state: &AgentLoopState, tools: &[ToolSpec]) -> ModelRequest {
    model_request_for_turn_with_system_prompt(state, tools, None)
}

pub fn model_request_for_turn_with_system_prompt(
    state: &AgentLoopState,
    tools: &[ToolSpec],
    system_prompt: Option<&str>,
) -> ModelRequest {
    let mut messages = vec![Message {
        role: MessageRole::System,
        content: agent_system_prompt_with_override(tools, system_prompt),
        metadata: Metadata::new(),
    }];
    messages.extend(state.messages.clone());

    ModelRequest {
        role: ModelRole::Executor,
        messages,
        tools: tools.to_vec(),
        mode: ModelCallMode::NonStreaming,
        metadata: [
            ("agent_task_id".to_string(), state.task_id.0.clone()),
            ("agent_turn".to_string(), state.turn.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

pub fn advance_with_model_response(
    state: &mut AgentLoopState,
    response: ModelResponse,
    tools: &[ToolSpec],
) -> AgentAdvance {
    state.turn += 1;

    let content = response.message.content.trim().to_string();
    let tool_call_count = response.tool_calls.len();
    if !content.is_empty() || tool_call_count > 0 {
        let mut metadata = response.message.metadata.clone();
        for (key, value) in response.metadata.clone() {
            metadata.entry(key).or_insert(value);
        }
        metadata.insert("tool_call_count".to_string(), tool_call_count.to_string());
        if let Some(raw_tool_calls_json) = response.raw_tool_calls_json.clone() {
            metadata.insert("raw_tool_calls_json".to_string(), raw_tool_calls_json);
        }
        if tool_call_count > 0 {
            metadata.insert(
                "tool_call_ids".to_string(),
                response
                    .tool_calls
                    .iter()
                    .map(|call| call.id.clone())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        state.messages.push(Message {
            role: MessageRole::Assistant,
            content: content.clone(),
            metadata,
        });
    }

    if state.turn > state.max_turns {
        return AgentAdvance::Failed {
            message: format!("agent exceeded max turn limit: {}", state.max_turns),
        };
    }

    if !response.tool_calls.is_empty() {
        let calls = response
            .tool_calls
            .into_iter()
            .map(|call| {
                let tool_name = original_tool_name(&call.name, tools);
                AgentToolRequest {
                    call_id: ToolCallId(call.id),
                    tool_name,
                    input: call.arguments_json,
                }
            })
            .collect::<Vec<_>>();

        if calls.is_empty() {
            return AgentAdvance::Failed {
                message: "model returned an empty tool call set".to_string(),
            };
        }

        return AgentAdvance::ToolCalls { calls };
    }

    AgentAdvance::Completed {
        answer: if content.is_empty() {
            "(model returned an empty answer)".to_string()
        } else {
            content
        },
    }
}

pub fn append_observation(state: &mut AgentLoopState, observation: &str) {
    state.messages.push(Message {
        role: MessageRole::User,
        content: format!("Tool observation:\n{observation}\n\nContinue the task. If complete, answer with a concise final summary."),
        metadata: [("kind".to_string(), "tool_observation".to_string())]
            .into_iter()
            .collect(),
    });
}

pub fn append_tool_observation(
    state: &mut AgentLoopState,
    tool_call_id: ToolCallId,
    observation: &str,
) {
    state.messages.push(Message {
        role: MessageRole::Tool,
        content: observation.to_string(),
        metadata: [
            ("kind".to_string(), "tool_observation".to_string()),
            ("tool_call_id".to_string(), tool_call_id.0),
        ]
        .into_iter()
        .collect(),
    });
}

pub fn tool_invocation_from_request(
    task_id: &TaskId,
    request: &AgentToolRequest,
) -> ToolInvocation {
    ToolInvocation {
        id: request.call_id.clone(),
        task_id: task_id.clone(),
        tool_name: request.tool_name.clone(),
        input_json: request.input.clone(),
        proposed_by_model: "agent-loop".to_string(),
        metadata: [("agent_generated".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
    }
}

pub fn observation_from_tool_result(tool_name: &str, status: &str, output: &str) -> String {
    format!(
        "tool={tool_name}\nstatus={status}\noutput=\n{}",
        truncate_observation(output)
    )
}

pub fn repeated_tool_failure_count(
    state: &AgentLoopState,
    tool_name: &str,
    input_json: &str,
) -> usize {
    state
        .failed_tool_signatures
        .get(&tool_signature(tool_name, input_json))
        .copied()
        .unwrap_or_default()
}

pub fn record_tool_outcome(
    state: &mut AgentLoopState,
    tool_name: &str,
    input_json: &str,
    status: &ToolOutcomeStatus,
) {
    let signature = tool_signature(tool_name, input_json);
    if matches!(status, ToolOutcomeStatus::Failed | ToolOutcomeStatus::Denied) {
        *state.failed_tool_signatures.entry(signature).or_default() += 1;
    } else {
        state.failed_tool_signatures.remove(&signature);
    }
}

fn tool_signature(tool_name: &str, input_json: &str) -> String {
    let canonical = serde_json::from_str::<serde_json::Value>(input_json)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| input_json.trim().to_string());
    format!("{tool_name}\n{canonical}")
}

pub fn agent_system_prompt(tools: &[ToolSpec]) -> String {
    agent_system_prompt_with_override(tools, None)
}

pub fn agent_system_prompt_with_override(
    tools: &[ToolSpec],
    system_prompt: Option<&str>,
) -> String {
    let configured = system_prompt.map(str::trim).filter(|prompt| !prompt.is_empty());
    let mut prompt = configured
        .unwrap_or(DEFAULT_AGENT_SYSTEM_PROMPT)
        .to_string();
    prompt.push_str("\n\nCindx runtime contract:\n");
    prompt.push_str("- Use local tools only through audited tool calls and never bypass permission checks.\n");
    prompt.push_str("- Use tools when local workspace information or state changes are required.\n");
    prompt.push_str("- Tool arguments must follow each function's JSON schema exactly.\n");
    prompt.push_str("- After tool observations, call another needed tool or provide a concise final answer.\n\n");
    prompt.push_str("Available tools:\n");
    for tool in tools {
        prompt.push_str(&format!(
            "- {} as function {}: {} Input schema: {}\n",
            tool.name,
            tool_function_name(&tool.name),
            tool.description,
            tool.input_schema_json.replace('\n', "; ")
        ));
    }
    prompt
}

fn original_tool_name(model_name: &str, tools: &[ToolSpec]) -> String {
    tools
        .iter()
        .find(|tool| tool.name == model_name || tool_function_name(&tool.name) == model_name)
        .map(|tool| tool.name.clone())
        .unwrap_or_else(|| model_name.replace('_', "."))
}

fn truncate_observation(output: &str) -> String {
    const LIMIT: usize = 6000;
    if output.chars().count() <= LIMIT {
        return output.to_string();
    }

    let mut truncated = output.chars().take(LIMIT).collect::<String>();
    truncated.push_str("\n...[truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{ToolRisk, ToolSpec};
    use model_provider::{ModelToolCall, ModelResponse};

    #[test]
    fn default_turn_budget_supports_multi_step_agent_runs() {
        assert_eq!(AgentRuntimeConfig::default().max_turns, 24);
    }

    #[test]
    fn repeated_identical_tool_failures_are_counted_by_canonical_arguments() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "test",
            AgentRuntimeConfig::default(),
        );
        record_tool_outcome(
            &mut state,
            "shell.run",
            r#"{"cwd":".","command":"false"}"#,
            &ToolOutcomeStatus::Failed,
        );
        record_tool_outcome(
            &mut state,
            "shell.run",
            r#"{"command":"false","cwd":"."}"#,
            &ToolOutcomeStatus::Failed,
        );

        assert_eq!(
            repeated_tool_failure_count(
                &state,
                "shell.run",
                r#"{"command":"false","cwd":"."}"#
            ),
            MAX_IDENTICAL_TOOL_FAILURES
        );
    }

    #[test]
    fn request_includes_system_prompt_and_tools() {
        let tools = vec![tool("file.read", "path=<workspace-relative-path>")];
        let state = start_agent_loop(
            TaskId("task-1".to_string()),
            "read README",
            AgentRuntimeConfig::default(),
        );

        let request = model_request_for_turn(&state, &tools);

        assert_eq!(request.mode, ModelCallMode::NonStreaming);
        assert_eq!(request.tools.len(), 1);
        assert!(request.messages[0].content.contains("file.read"));
        assert!(request.messages[0].content.contains("file_read"));
    }

    #[test]
    fn custom_system_prompt_keeps_runtime_contract_and_tools() {
        let tools = vec![tool("file.read", "path=<workspace-relative-path>")];
        let state = start_agent_loop(
            TaskId("task-1".to_string()),
            "read README",
            AgentRuntimeConfig::default(),
        );

        let request = model_request_for_turn_with_system_prompt(
            &state,
            &tools,
            Some("Answer in Chinese and cite evidence."),
        );
        let prompt = &request.messages[0].content;

        assert!(prompt.starts_with("Answer in Chinese and cite evidence."));
        assert!(prompt.contains("never bypass permission checks"));
        assert!(prompt.contains("JSON schema exactly"));
        assert!(prompt.contains("file.read"));
    }

    #[test]
    fn model_tool_call_advances_to_tool_request() {
        let tools = vec![tool("file.read", "path=<workspace-relative-path>")];
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "read README",
            AgentRuntimeConfig::default(),
        );
        let response = ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: String::new(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: Some("[]".to_string()),
            tool_calls: vec![ModelToolCall {
                id: "call-1".to_string(),
                name: "file_read".to_string(),
                arguments_json: r#"{"input":"path=README.md"}"#.to_string(),
            }],
            metadata: Metadata::new(),
        };

        let advance = advance_with_model_response(&mut state, response, &tools);

        match advance {
            AgentAdvance::ToolCalls { calls } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].tool_name, "file.read");
                assert_eq!(calls[0].input, r#"{"input":"path=README.md"}"#);
            }
            other => panic!("unexpected advance: {other:?}"),
        }
    }

    #[test]
    fn observations_resume_as_user_context() {
        let mut state = resume_agent_loop(
            TaskId("task-1".to_string()),
            "read README",
            &["tool=file.read\nstatus=succeeded\noutput=hello".to_string()],
            AgentRuntimeConfig::default(),
        );
        let response = ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: "README says hello".to_string(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: Vec::new(),
            metadata: Metadata::new(),
        };

        let advance = advance_with_model_response(&mut state, response, &[]);

        assert!(matches!(advance, AgentAdvance::Completed { .. }));
        assert!(state.messages.iter().any(|message| message.content.contains("Tool observation")));
    }

    #[test]
    fn canonical_transcript_resumes_with_tool_role() {
        let mut assistant_metadata = Metadata::new();
        assistant_metadata.insert(
            "raw_tool_calls_json".to_string(),
            r#"[{"id":"call-1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]"#.to_string(),
        );
        let state = resume_agent_loop_from_messages(
            TaskId("task-1".to_string()),
            "read README",
            vec![
                Message {
                    role: MessageRole::User,
                    content: "read README".to_string(),
                    metadata: Metadata::new(),
                },
                Message {
                    role: MessageRole::Assistant,
                    content: String::new(),
                    metadata: assistant_metadata,
                },
                Message {
                    role: MessageRole::Tool,
                    content: "tool=file.read\nstatus=succeeded\noutput=hello".to_string(),
                    metadata: [("tool_call_id".to_string(), "call-1".to_string())]
                        .into_iter()
                        .collect(),
                },
            ],
            AgentRuntimeConfig::default(),
        );

        assert_eq!(state.turn, 1);
        assert!(state
            .messages
            .iter()
            .any(|message| matches!(message.role, MessageRole::Tool)
                && message.metadata.get("tool_call_id").map(String::as_str) == Some("call-1")));
    }

    #[test]
    fn history_starts_a_fresh_turn_without_losing_messages() {
        let history = vec![
            Message {
                role: MessageRole::User,
                content: "first question".to_string(),
                metadata: Metadata::new(),
            },
            Message {
                role: MessageRole::Assistant,
                content: "first answer".to_string(),
                metadata: Metadata::new(),
            },
        ];

        let state = start_agent_loop_with_history(
            TaskId("task-1".to_string()),
            "follow up",
            history,
            AgentRuntimeConfig::default(),
        );

        assert_eq!(state.turn, 0);
        assert_eq!(state.messages.len(), 3);
        assert_eq!(state.messages[2].content, "follow up");
    }

    fn tool(name: &str, schema: &str) -> ToolSpec {
        let schema = if schema.trim_start().starts_with('{') {
            schema.to_string()
        } else {
            r#"{"type":"object","properties":{"path":{"type":"string","description":"workspace-relative path"}},"required":["path"],"additionalProperties":false}"#.to_string()
        };
        ToolSpec::builtin(
            name,
            name.split('.').next().unwrap_or("test"),
            format!("{name} description"),
            ToolRisk::ReadOnly,
            schema,
        )
    }
}
