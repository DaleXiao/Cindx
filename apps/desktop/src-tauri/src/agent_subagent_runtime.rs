use crate::agent_query_commands::{agent_run_should_stop, append_agent_progress_event};
use crate::app_state::AppState;
use agent_core::{Message, MessageRole, Metadata, ModelRole, TaskId, ToolOutcomeStatus, ToolSpec};
use agent_runtime::{
    observation_from_tool_result, subagent_tool_allowed, tool_invocation_from_request,
    AgentRunControl, AgentToolRequest, RunStageClass, SUBAGENT_MAX_STEPS,
};
use model_provider::{ModelCallMode, ModelRequest, StreamingModelProvider};
use std::sync::Arc;
use tools::ToolRegistry;

/// Split a tool-call batch, run any `task` delegations as isolated child
/// completions (returning their answers as internal instructions), and hand back
/// the remaining normal calls for regular execution. Delegations run concurrently
/// (preserving call order), honour run cancellation, and emit transient
/// started/finished progress events so the UI can show each subagent working.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_subagent_delegations(
    runtime: &mut agent_runtime::AgentLoopState,
    actor_provider: &dyn StreamingModelProvider,
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    cancellation: &Arc<AgentRunControl>,
    registry: &ToolRegistry,
    calls: Vec<agent_runtime::AgentToolRequest>,
) -> Vec<agent_runtime::AgentToolRequest> {
    let (task_calls, normal_calls): (Vec<_>, Vec<_>) =
        calls.into_iter().partition(|call| call.tool_name == "task");
    if task_calls.is_empty() {
        return normal_calls;
    }
    for call in &task_calls {
        let description = subagent_description(&call.input);
        let _ = append_agent_progress_event(
            state,
            &runtime.task_id,
            run_context,
            &format!("Subagent started: {description}"),
        );
    }
    // Resolve the read-only subagent tool surface once, before spawning children,
    // so every child sees the same bounded whitelist.
    let subagent_tools = subagent_tool_specs(registry);
    let task_id = runtime.task_id.clone();
    let answers: Vec<(String, String)> = std::thread::scope(|scope| {
        let handles: Vec<_> = task_calls
            .iter()
            .map(|call| {
                scope.spawn(|| {
                    subagent_child_answer(
                        actor_provider,
                        &call.input,
                        cancellation,
                        registry,
                        &subagent_tools,
                        &task_id,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| match handle.join() {
                Ok(answer) => answer,
                Err(_) => (
                    "delegated task".to_string(),
                    "Subagent did not complete.".to_string(),
                ),
            })
            .collect()
    });
    for (call, (description, answer)) in task_calls.iter().zip(answers) {
        strip_subagent_call_id(runtime, &call.call_id.0);
        agent_runtime::append_internal_instruction(
            runtime,
            "subagent_result",
            &format!("Subagent result for {description:?}:\n{answer}"),
        );
        let _ = append_agent_progress_event(
            state,
            &runtime.task_id,
            run_context,
            &format!("Subagent finished: {description}"),
        );
    }
    normal_calls
}

fn subagent_description(input_json: &str) -> String {
    let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap_or_default();
    input
        .get("description")
        .and_then(|value| value.as_str())
        .unwrap_or("delegated task")
        .trim()
        .to_string()
}

/// The read-only tool surface a subagent may call, resolved from the registry
/// and filtered through the deterministic whitelist so the spec and the
/// execution policy can never drift apart.
fn subagent_tool_specs(registry: &ToolRegistry) -> Vec<ToolSpec> {
    registry
        .specs()
        .into_iter()
        .filter(|spec| subagent_tool_allowed(&spec.name))
        .collect()
}

/// Run a bounded, isolated child run for a `task` delegation and return
/// (description, answer). The child sees only the delegated prompt (context
/// isolation) plus the subagent contract; it never sees the parent transcript.
/// Unlike a plain completion, the child runs a bounded read-only tool loop: each
/// step it may call whitelisted read-only tools, whose observations are appended
/// to its own message history, until it answers without a tool call or exhausts
/// `SUBAGENT_MAX_STEPS`. Every model call is charged to the parent run's Worker
/// stage budget, and the child honours the parent run's cancellation so a
/// stopped run aborts it.
pub(crate) fn subagent_child_answer(
    actor_provider: &dyn StreamingModelProvider,
    input_json: &str,
    cancellation: &Arc<AgentRunControl>,
    registry: &ToolRegistry,
    subagent_tools: &[ToolSpec],
    task_id: &TaskId,
) -> (String, String) {
    let description = subagent_description(input_json);
    let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap_or_default();
    let detail = input.get("prompt").and_then(|value| value.as_str()).unwrap_or("");
    let child_prompt = agent_runtime::build_subagent_task_prompt(&description, detail);
    let mut messages = vec![
        Message {
            role: MessageRole::System,
            content: agent_runtime::subagent_system_prompt().to_string(),
            metadata: Metadata::new(),
        },
        Message {
            role: MessageRole::User,
            content: child_prompt,
            metadata: Metadata::new(),
        },
    ];
    let mut last_content = String::new();
    for _step in 0..SUBAGENT_MAX_STEPS {
        if agent_run_should_stop(cancellation) {
            return (description, subagent_stopped_answer(&last_content));
        }
        // Charge the model call to the parent run's Worker stage budget; an
        // exhausted stage budget stops the child loop without stopping the run.
        if cancellation
            .begin_stage_model_call("subagent", RunStageClass::Worker)
            .is_err()
        {
            return (description, subagent_budget_answer(&last_content));
        }
        let request = ModelRequest {
            role: ModelRole::Executor,
            messages: messages.clone(),
            tools: subagent_tools.to_vec(),
            mode: ModelCallMode::NonStreaming,
            metadata: Metadata::new(),
        };
        let mut should_cancel = || agent_run_should_stop(cancellation);
        let response = actor_provider.complete_streaming_cancellable(
            request,
            &mut |_| {},
            &mut should_cancel,
        );
        cancellation.finish_model_call();
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                return (
                    description,
                    if last_content.is_empty() {
                        "Subagent could not run (provider unavailable).".to_string()
                    } else {
                        last_content
                    },
                )
            }
        };
        last_content = response.message.content.clone();
        if response.tool_calls.is_empty() {
            return (description, last_content);
        }
        // Record the assistant turn (with its raw tool calls) so the next
        // request payload is well-formed, then execute each read-only call.
        let mut assistant_metadata = Metadata::new();
        if let Some(raw) = response.raw_tool_calls_json.clone() {
            assistant_metadata.insert("raw_tool_calls_json".to_string(), raw);
        }
        assistant_metadata.insert(
            "tool_call_ids".to_string(),
            response
                .tool_calls
                .iter()
                .map(|call| call.id.clone())
                .collect::<Vec<_>>()
                .join(","),
        );
        messages.push(Message {
            role: MessageRole::Assistant,
            content: response.message.content.clone(),
            metadata: assistant_metadata,
        });
        for call in &response.tool_calls {
            let observation = execute_subagent_tool_call(registry, task_id, call);
            messages.push(Message {
                role: MessageRole::Tool,
                content: observation,
                metadata: [
                    ("kind".to_string(), "tool_observation".to_string()),
                    ("tool_call_id".to_string(), call.id.clone()),
                ]
                .into_iter()
                .collect(),
            });
        }
    }
    (description, subagent_step_limit_answer(&last_content))
}

/// Execute one whitelisted read-only subagent tool call and format its
/// observation. The whitelist and the registry's read-only/permissionless guard
/// are both enforced, so a disallowed or effectful call becomes an observation
/// rather than an execution.
fn execute_subagent_tool_call(
    registry: &ToolRegistry,
    task_id: &TaskId,
    call: &agent_core::ModelToolCall,
) -> String {
    let request = AgentToolRequest {
        call_id: agent_core::ToolCallId(call.id.clone()),
        tool_name: call.name.clone(),
        input: call.arguments_json.clone(),
    };
    if !subagent_tool_allowed(&call.name) {
        return observation_from_tool_result(
            &call.name,
            "denied",
            "tool is not permitted for a subagent (read-only discovery only)",
        );
    }
    let invocation = tool_invocation_from_request(task_id, &request);
    let result = match registry.permissionless_read_tool(&invocation) {
        Ok(tool) => tool.execute(invocation).unwrap_or_else(|error| {
            agent_core::ToolResult::failed(agent_core::ToolCallId(call.id.clone()), error.message)
        }),
        Err(error) => {
            agent_core::ToolResult::failed(agent_core::ToolCallId(call.id.clone()), error.message)
        }
    };
    let status = match result.status {
        ToolOutcomeStatus::Succeeded => "succeeded",
        ToolOutcomeStatus::Failed => "failed",
        ToolOutcomeStatus::Cancelled => "cancelled",
        ToolOutcomeStatus::Denied => "denied",
    };
    observation_from_tool_result(&call.name, status, &result.output)
}

fn subagent_stopped_answer(partial: &str) -> String {
    if partial.trim().is_empty() {
        "Subagent stopped before producing an answer.".to_string()
    } else {
        format!("{partial}\n\n[subagent stopped by run cancellation]")
    }
}

fn subagent_budget_answer(partial: &str) -> String {
    if partial.trim().is_empty() {
        "Subagent exhausted its stage budget before producing an answer.".to_string()
    } else {
        format!("{partial}\n\n[subagent stopped: stage budget exhausted]")
    }
}

fn subagent_step_limit_answer(partial: &str) -> String {
    if partial.trim().is_empty() {
        "Subagent reached its step limit without a final answer.".to_string()
    } else {
        format!("{partial}\n\n[subagent stopped: step limit {SUBAGENT_MAX_STEPS} reached]")
    }
}

/// Remove a delegated `task` call id from the latest assistant message so the
/// transcript has no dangling tool call (the answer is returned as an internal
/// instruction instead of a tool observation).
pub(crate) fn strip_subagent_call_id(
    runtime: &mut agent_runtime::AgentLoopState,
    call_id: &str,
) {
    let Some(message) = runtime
        .messages
        .iter_mut()
        .rev()
        .find(|message| message.role == MessageRole::Assistant)
    else {
        return;
    };
    if let Some(ids) = message.metadata.get("tool_call_ids").cloned() {
        let kept = ids
            .split(',')
            .filter(|id| id.trim() != call_id)
            .collect::<Vec<_>>()
            .join(",");
        if kept.is_empty() {
            message.metadata.remove("tool_call_ids");
        } else {
            message.metadata.insert("tool_call_ids".to_string(), kept);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::ModelToolCall;
    use model_provider::ModelResponse;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A scripted provider that replays a fixed queue of responses and counts
    /// how many model calls it served, so tests can drive a deterministic
    /// subagent loop without a network.
    struct ScriptedProvider {
        responses: Mutex<VecDeque<ModelResponse>>,
        calls: AtomicUsize,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<ModelResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                calls: AtomicUsize::new(0),
            }
        }

        fn served(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl StreamingModelProvider for ScriptedProvider {
        fn complete_streaming_cancellable(
            &self,
            _request: ModelRequest,
            _on_delta: &mut dyn FnMut(&str),
            should_cancel: &mut dyn FnMut() -> bool,
        ) -> Result<ModelResponse, model_provider::ModelError> {
            if should_cancel() {
                return Err(model_provider::ModelError::new("cancelled"));
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses
                .lock()
                .expect("scripted provider poisoned")
                .pop_front()
                .ok_or_else(|| model_provider::ModelError::new("script exhausted"))
        }
    }

    fn final_answer(text: &str) -> ModelResponse {
        ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: text.to_string(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: Vec::new(),
            metadata: Metadata::new(),
        }
    }

    fn tool_call_step(call: ModelToolCall) -> ModelResponse {
        ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: String::new(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: Some(format!(
                "[{{\"id\":\"{}\",\"function\":{{\"name\":\"{}\",\"arguments\":{}}}}}]",
                call.id, call.name, call.arguments_json
            )),
            tool_calls: vec![call],
            metadata: Metadata::new(),
        }
    }

    fn read_call(id: &str, path: &str) -> ModelToolCall {
        ModelToolCall {
            id: id.to_string(),
            name: "file.read".to_string(),
            arguments_json: format!("{{\"path\":\"{path}\"}}"),
        }
    }

    fn fixture() -> (tempfile::TempDir, ToolRegistry, TaskId) {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        std::fs::write(workspace.path().join("README.md"), "line one\nline two\n")
            .expect("seed README");
        let registry = ToolRegistry::with_workspace_tools(workspace.path());
        (workspace, registry, TaskId("subagent-test".to_string()))
    }

    fn delegation_input() -> String {
        r#"{"description":"inspect readme","prompt":"read README.md"}"#.to_string()
    }

    #[test]
    fn subagent_runs_read_only_loop_to_a_final_answer() {
        let (_workspace, registry, task_id) = fixture();
        let provider = ScriptedProvider::new(vec![
            tool_call_step(read_call("c1", "README.md")),
            final_answer("README.md:1 says line one"),
        ]);
        let control = Arc::new(AgentRunControl::new("fast"));
        let tools = subagent_tool_specs(&registry);

        let (_description, answer) = subagent_child_answer(
            &provider,
            &delegation_input(),
            &control,
            &registry,
            &tools,
            &task_id,
        );

        assert_eq!(answer, "README.md:1 says line one");
        assert_eq!(provider.served(), 2);
    }

    #[test]
    fn subagent_loop_is_bounded_by_step_limit() {
        let (_workspace, registry, task_id) = fixture();
        // The provider always demands another tool call, so only the step limit
        // can stop the loop.
        let provider = ScriptedProvider::new(vec![
            tool_call_step(read_call("loop", "README.md"));
            SUBAGENT_MAX_STEPS + 3
        ]);
        let control = Arc::new(AgentRunControl::new("fast"));
        let tools = subagent_tool_specs(&registry);

        let (_description, answer) = subagent_child_answer(
            &provider,
            &delegation_input(),
            &control,
            &registry,
            &tools,
            &task_id,
        );

        // The loop never exceeds the step budget even with an infinite tool-call
        // stream, and the answer carries the step-limit marker.
        assert!(provider.served() <= SUBAGENT_MAX_STEPS);
        assert!(answer.contains("step limit") || answer.contains("budget"));
    }

    #[test]
    fn subagent_read_tool_executes_and_returns_line_addressable_content() {
        let (_workspace, registry, task_id) = fixture();
        let observation =
            execute_subagent_tool_call(&registry, &task_id, &read_call("r1", "README.md"));
        assert!(observation.contains("tool=file.read"));
        assert!(observation.contains("status=succeeded"));
        assert!(observation.contains("line one"));
    }

    #[test]
    fn subagent_denies_effectful_tool() {
        let (_workspace, registry, task_id) = fixture();
        let write_call = ModelToolCall {
            id: "w1".to_string(),
            name: "file.write".to_string(),
            arguments_json: r#"{"path":"evil.txt","content":"x"}"#.to_string(),
        };
        let observation = execute_subagent_tool_call(&registry, &task_id, &write_call);
        assert!(observation.contains("status=denied"));
        // The effectful tool never ran.
        assert!(!_workspace.path().join("evil.txt").exists());
    }

    #[test]
    fn subagent_aborts_before_any_model_call_when_cancelled() {
        let (_workspace, registry, task_id) = fixture();
        let provider = ScriptedProvider::new(vec![final_answer("should not run")]);
        let control = Arc::new(AgentRunControl::new("fast"));
        control.request_cancel();
        let tools = subagent_tool_specs(&registry);

        let (_description, answer) = subagent_child_answer(
            &provider,
            &delegation_input(),
            &control,
            &registry,
            &tools,
            &task_id,
        );

        assert!(answer.contains("stopped"));
        assert_eq!(provider.served(), 0);
    }

    #[test]
    fn subagent_model_calls_draw_from_the_shared_worker_stage_budget() {
        let (_workspace, registry, task_id) = fixture();
        let control = Arc::new(AgentRunControl::new("fast"));
        // Exhaust the Worker stage budget (bounded at max_model_calls/2 for the
        // tier) so the child's first charged call must fail.
        let mut exhausted = false;
        for _ in 0..32 {
            if control
                .begin_stage_model_call("subagent", RunStageClass::Worker)
                .is_err()
            {
                exhausted = true;
                break;
            }
        }
        assert!(exhausted, "worker stage budget should be bounded");

        let provider = ScriptedProvider::new(vec![final_answer("should not run")]);
        let tools = subagent_tool_specs(&registry);
        let (_description, answer) = subagent_child_answer(
            &provider,
            &delegation_input(),
            &control,
            &registry,
            &tools,
            &task_id,
        );

        assert!(answer.contains("budget"));
        assert_eq!(provider.served(), 0);
    }

    #[test]
    fn subagent_tool_surface_is_the_read_only_whitelist() {
        let (_workspace, registry, _task_id) = fixture();
        let names: Vec<String> = subagent_tool_specs(&registry)
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        assert!(names.iter().any(|name| name == "file.read"));
        assert!(!names.iter().any(|name| name == "file.write"));
        assert!(!names.iter().any(|name| name == "shell.run"));
    }
}
