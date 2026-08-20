use crate::agent_query_commands::{agent_run_should_stop, append_agent_progress_event};
use crate::app_state::AppState;
use agent_core::{Message, MessageRole, Metadata, ModelRole};
use agent_runtime::AgentRunControl;
use model_provider::{ModelCallMode, ModelRequest, StreamingModelProvider};
use std::sync::Arc;

/// Split a tool-call batch, run any `task` delegations as isolated child
/// completions (returning their answers as internal instructions), and hand back
/// the remaining normal calls for regular execution. Delegations run concurrently
/// (preserving call order), honour run cancellation, and emit transient
/// started/finished progress events so the UI can show each subagent working.
pub(crate) fn execute_subagent_delegations(
    runtime: &mut agent_runtime::AgentLoopState,
    actor_provider: &dyn StreamingModelProvider,
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    cancellation: &Arc<AgentRunControl>,
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
    let answers: Vec<(String, String)> = std::thread::scope(|scope| {
        let handles: Vec<_> = task_calls
            .iter()
            .map(|call| {
                scope.spawn(|| subagent_child_answer(actor_provider, &call.input, cancellation))
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

/// Run a bounded, isolated child completion for a `task` delegation and return
/// (description, answer). The child sees only the delegated prompt (context
/// isolation) plus the subagent contract; it never sees the parent transcript.
/// The child honours the parent run's cancellation so a stopped run aborts it.
pub(crate) fn subagent_child_answer(
    actor_provider: &dyn StreamingModelProvider,
    input_json: &str,
    cancellation: &Arc<AgentRunControl>,
) -> (String, String) {
    let description = subagent_description(input_json);
    let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap_or_default();
    let detail = input.get("prompt").and_then(|value| value.as_str()).unwrap_or("");
    let child_prompt = agent_runtime::build_subagent_task_prompt(&description, detail);
    let request = ModelRequest {
        role: ModelRole::Executor,
        messages: vec![
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
        ],
        tools: Vec::new(),
        mode: ModelCallMode::NonStreaming,
        metadata: Metadata::new(),
    };
    let mut should_cancel = || agent_run_should_stop(cancellation);
    let answer = match actor_provider.complete_streaming_cancellable(
        request,
        &mut |_| {},
        &mut should_cancel,
    ) {
        Ok(response) => response.message.content,
        Err(_) => "Subagent could not run (provider unavailable).".to_string(),
    };
    (description, answer)
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
