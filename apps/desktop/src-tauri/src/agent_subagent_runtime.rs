use crate::agent_query_commands::{agent_run_should_stop, append_agent_progress_event};
use crate::app_state::AppState;
use agent_application::{insert_run_objectives, merge_persistable_run_context};
use agent_core::{
    Message, MessageRole, Metadata, ModelRole, PermissionDecision, PermissionRequest,
    PermissionRequestId, PermissionResolution, TaskId, ToolCallId, ToolInvocation,
    ToolOutcomeStatus, ToolSpec,
};
use agent_runtime::{
    observation_from_agent_tool_result, observation_from_tool_result, prompt_completion_intent,
    subagent_patch_tool_allowed, subagent_tool_allowed, subagent_write_system_prompt,
    tool_input_fingerprint, tool_invocation_from_request, AgentRunControl, AgentToolRequest,
    PromptEffectAuthority, RunStageClass, SUBAGENT_MAX_STEPS,
};
use agent_storage::{PermissionStore, SqliteStore};
use model_provider::{ModelCallMode, ModelRequest, StreamingModelProvider};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tools::ToolRegistry;

/// Metadata marker on a permission request created by a write subagent's
/// patch call. The permission resolver uses it to resolve the request in
/// place (the subagent thread is parked waiting on the decision) instead of
/// resuming a suspended run.
pub(crate) const SUBAGENT_PERMISSION_ORIGIN_KEY: &str = "subagent";
pub(crate) const SUBAGENT_PERMISSION_ORIGIN_VALUE: &str = "true";
const SUBAGENT_PERMISSION_POLL_INTERVAL: Duration = Duration::from_millis(150);
const SUBAGENT_DESCRIPTION_MAX_CHARS: usize = 80;

/// Split a tool-call batch, run any `task` delegations as isolated child
/// completions (returning their answers as internal instructions), and hand back
/// the remaining normal calls for regular execution. Delegations run concurrently
/// (preserving call order), honour run cancellation, and emit transient
/// started/finished progress events so the UI can show each subagent working.
/// A delegation may request `allow_patches`; the write surface is granted only
/// when the parent run's own prompt effect authority permits workspace effects.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_subagent_delegations(
    runtime: &mut agent_runtime::AgentLoopState,
    actor_provider: &dyn StreamingModelProvider,
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    cancellation: &Arc<AgentRunControl>,
    registry: &ToolRegistry,
    workspace_root: &Path,
    calls: Vec<agent_runtime::AgentToolRequest>,
) -> Vec<agent_runtime::AgentToolRequest> {
    let (task_calls, normal_calls): (Vec<_>, Vec<_>) =
        calls.into_iter().partition(|call| call.tool_name == "task");
    if task_calls.is_empty() {
        return normal_calls;
    }
    let write_modes: Vec<SubagentWriteMode> = task_calls
        .iter()
        .map(|call| subagent_write_mode(run_context, &call.input))
        .collect();
    for (call, write_mode) in task_calls.iter().zip(&write_modes) {
        let description = subagent_description(&call.input);
        let summary = match write_mode {
            SubagentWriteMode::ReadOnly => format!("Subagent started: {description}"),
            SubagentWriteMode::Write => format!("Write subagent started: {description}"),
            SubagentWriteMode::Refused => {
                format!("Write subagent refused: {description} (run forbids workspace effects)")
            }
        };
        let _ = append_agent_progress_event(state, &runtime.task_id, run_context, &summary);
    }
    // Resolve the subagent tool surfaces once, before spawning children, so
    // every child sees the same bounded whitelist.
    let read_only_tools = subagent_tool_specs_for_mode(registry, false);
    let write_tools = subagent_tool_specs_for_mode(registry, true);
    let task_id = runtime.task_id.clone();
    // Context fork: seed each child with the parent's balanced completed-round
    // prefix (bounded, in-flight rounds excluded) ahead of the subagent system
    // and delegation prompts.
    let context_prefix = agent_runtime::subagent_context_fork_prefix(
        &runtime.messages,
        agent_runtime::SUBAGENT_CONTEXT_FORK_MAX_MESSAGES,
    );
    let answers: Vec<(String, String)> = std::thread::scope(|scope| {
        let handles: Vec<_> = task_calls
            .iter()
            .zip(&write_modes)
            .map(|(call, write_mode)| {
                if matches!(write_mode, SubagentWriteMode::Refused) {
                    // A refused write delegation never starts a child: it costs
                    // no model call and its answer states the refusal.
                    return None;
                }
                let write_context = match write_mode {
                    SubagentWriteMode::Write => Some(SubagentWriteContext {
                        state,
                        run_context,
                        workspace_root,
                        parent_call_id: &call.call_id.0,
                    }),
                    _ => None,
                };
                let tools = if write_context.is_some() {
                    &write_tools
                } else {
                    &read_only_tools
                };
                Some(scope.spawn(|| {
                    subagent_child_answer(
                        actor_provider,
                        &call.input,
                        cancellation,
                        registry,
                        tools,
                        &task_id,
                        write_context,
                        &context_prefix,
                    )
                }))
            })
            .collect();
        handles
            .into_iter()
            .zip(&task_calls)
            .map(|(handle, call)| match handle {
                Some(handle) => match handle.join() {
                    Ok(answer) => answer,
                    Err(_) => (
                        "delegated task".to_string(),
                        "Subagent did not complete.".to_string(),
                    ),
                },
                None => (
                    subagent_description(&call.input),
                    subagent_write_refused_answer(),
                ),
            })
            .collect()
    });
    for ((call, write_mode), (description, answer)) in
        task_calls.iter().zip(&write_modes).zip(answers)
    {
        strip_subagent_call_id(runtime, &call.call_id.0);
        agent_runtime::append_internal_instruction(
            runtime,
            "subagent_result",
            &format!("Subagent result for {description:?}:\n{answer}"),
        );
        let summary = match write_mode {
            SubagentWriteMode::ReadOnly => Some(format!("Subagent finished: {description}")),
            SubagentWriteMode::Write => Some(format!("Write subagent finished: {description}")),
            // The refusal was already announced when the delegation arrived.
            SubagentWriteMode::Refused => None,
        };
        if let Some(summary) = summary {
            let _ = append_agent_progress_event(state, &runtime.task_id, run_context, &summary);
        }
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

/// Whether a `task` delegation asked for the patch-capable write surface.
/// Missing or malformed values default to a read-only subagent.
fn subagent_allow_patches_requested(input_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(input_json)
        .ok()
        .and_then(|input| input.get("allow_patches").and_then(|value| value.as_bool()))
        .unwrap_or(false)
}

/// The effective write mode of one delegation. `allow_patches` takes effect
/// only when the parent run's own prompt effect authority permits workspace
/// effects: a run whose objective forbids effects (for example an explicit
/// read-only request) refuses the write surface fail-closed, and the refusal
/// costs no model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubagentWriteMode {
    ReadOnly,
    Write,
    Refused,
}

fn subagent_write_mode(run_context: &Metadata, input_json: &str) -> SubagentWriteMode {
    if !subagent_allow_patches_requested(input_json) {
        return SubagentWriteMode::ReadOnly;
    }
    if prompt_completion_intent(run_context).effect_authority == PromptEffectAuthority::Forbidden {
        return SubagentWriteMode::Refused;
    }
    SubagentWriteMode::Write
}

fn subagent_write_refused_answer() -> String {
    "Subagent not started: `allow_patches` was requested, but this run's objective forbids workspace effects. Re-delegate without `allow_patches` for read-only work, or have the user lift the read-only constraint.".to_string()
}

/// The tool surface a subagent may call, resolved from the registry and
/// filtered through the deterministic whitelist so the spec and the execution
/// policy can never drift apart. Write mode adds exactly the two patch tools
/// (`file.patch`, `file.patch_batch`); every other write/effect tool stays
/// excluded.
fn subagent_tool_specs_for_mode(registry: &ToolRegistry, write: bool) -> Vec<ToolSpec> {
    registry
        .specs()
        .into_iter()
        .filter(|spec| {
            subagent_tool_allowed(&spec.name) || (write && subagent_patch_tool_allowed(&spec.name))
        })
        .collect()
}

/// Everything a write-mode child needs to route a patch call through the
/// parent run's permission path. The child never holds a grant of its own:
/// each patch call is persisted under the parent run identity and waits for
/// an explicit per-call user decision.
pub(crate) struct SubagentWriteContext<'a> {
    pub(crate) state: &'a tauri::State<'a, AppState>,
    pub(crate) run_context: &'a Metadata,
    pub(crate) workspace_root: &'a Path,
    /// The parent batch's `task` call id; namespaces the child's tool call
    /// ids so concurrent siblings cannot collide in the durable event log.
    pub(crate) parent_call_id: &'a str,
}

/// Run a bounded, isolated child run for a `task` delegation and return
/// (description, answer). The child is seeded with `parent_context` — the
/// parent run's balanced completed-round prefix (bounded, in-flight rounds
/// excluded) — placed ahead of the subagent system prompt and the delegated
/// task prompt; an empty prefix keeps the former fully isolated shape. Unlike
/// a plain completion, the child runs a bounded read-only tool loop: each step
/// it may call whitelisted read-only tools, whose observations are appended to
/// its own message history, until it answers without a tool call or exhausts
/// `SUBAGENT_MAX_STEPS`. Every model call is charged to the parent run's
/// Worker stage budget, and the child honours the parent run's cancellation so
/// a stopped run aborts it. With a `SubagentWriteContext` the child may
/// additionally attempt `file.patch`/`file.patch_batch`; each such call parks
/// on an explicit user approval routed through the parent run's permission
/// path (session grants never apply).
#[allow(clippy::too_many_arguments)]
pub(crate) fn subagent_child_answer(
    actor_provider: &dyn StreamingModelProvider,
    input_json: &str,
    cancellation: &Arc<AgentRunControl>,
    registry: &ToolRegistry,
    subagent_tools: &[ToolSpec],
    task_id: &TaskId,
    write: Option<SubagentWriteContext<'_>>,
    parent_context: &[Message],
) -> (String, String) {
    let description = subagent_description(input_json);
    let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap_or_default();
    let detail = input
        .get("prompt")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let child_prompt = agent_runtime::build_subagent_task_prompt(&description, detail);
    let system_prompt = if write.is_some() {
        subagent_write_system_prompt()
    } else {
        agent_runtime::subagent_system_prompt()
    };
    let mut messages = parent_context.to_vec();
    messages.push(Message {
        role: MessageRole::System,
        content: system_prompt.to_string(),
        metadata: Metadata::new(),
    });
    messages.push(Message {
        role: MessageRole::User,
        content: child_prompt,
        metadata: Metadata::new(),
    });
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
            // The child dispatches through `complete_streaming_cancellable`, so
            // declare the streaming mode the transport actually runs.
            mode: ModelCallMode::Streaming,
            metadata: Metadata::new(),
        };
        let mut should_cancel = || agent_run_should_stop(cancellation);
        let response =
            actor_provider.complete_streaming_cancellable(request, &mut |_| {}, &mut should_cancel);
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
            let observation = match &write {
                Some(context) if subagent_patch_tool_allowed(&call.name) => {
                    execute_write_subagent_tool_call(
                        context,
                        registry,
                        task_id,
                        call,
                        cancellation,
                        &description,
                    )
                }
                _ => execute_subagent_tool_call(registry, task_id, call),
            };
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

/// The decision a write subagent's parked patch approval resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubagentPermissionOutcome {
    Approved,
    Denied,
    Cancelled,
}

/// Persist one write subagent patch call as a pending permission request of
/// the PARENT run. The request carries the run's identity (task, session,
/// agent run, objectives) so the ordinary permission UI and audit trail apply
/// unchanged, but it is marked `session_reusable=false` and subagent-originated:
/// no session capability can satisfy it (`permission_can_allow_session` is
/// false), an `allow_for_session` decision is rejected, and the permission
/// resolver answers it in place instead of resuming a suspended run.
fn request_subagent_patch_approval(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    invocation: &ToolInvocation,
    mut request: PermissionRequest,
    description: &str,
) -> Result<PermissionRequestId, String> {
    let bounded_description: String = description
        .chars()
        .take(SUBAGENT_DESCRIPTION_MAX_CHARS)
        .collect();
    request.reason = format!(
        "Write subagent \"{bounded_description}\" requests approval: {}",
        request.reason
    );
    request
        .metadata
        .insert("phase".to_string(), "16".to_string());
    request
        .metadata
        .entry("tool_input".to_string())
        .or_insert_with(|| invocation.input_json.clone());
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0.clone());
    request
        .metadata
        .entry("tool_name".to_string())
        .or_insert_with(|| invocation.tool_name.clone());
    request.metadata.insert(
        "tool_input_fingerprint".to_string(),
        tool_input_fingerprint(&invocation.tool_name, &invocation.input_json),
    );
    request
        .metadata
        .insert("session_reusable".to_string(), "false".to_string());
    request.metadata.insert(
        SUBAGENT_PERMISSION_ORIGIN_KEY.to_string(),
        SUBAGENT_PERMISSION_ORIGIN_VALUE.to_string(),
    );
    request
        .metadata
        .insert("subagent_description".to_string(), bounded_description);
    request.metadata = merge_persistable_run_context(request.metadata, run_context);
    insert_run_objectives(&mut request.metadata, run_context);
    request.id = PermissionRequestId(crate::runtime_values::unique_id("agent-perm"));
    let request_id = request.id.clone();
    store
        .save_permission_request(
            request.clone(),
            crate::runtime_values::current_time_millis(),
        )
        .map_err(|error| error.to_string())?;
    let mut permission_metadata = [
        ("permission_id".to_string(), request_id.0.clone()),
        ("tool_call_id".to_string(), invocation.id.0.clone()),
        ("tool".to_string(), request.action.clone()),
        (
            "risk".to_string(),
            crate::permission_service::permission_risk_label(&request.risk).to_string(),
        ),
        ("scope".to_string(), request.scope.clone()),
        (
            SUBAGENT_PERMISSION_ORIGIN_KEY.to_string(),
            SUBAGENT_PERMISSION_ORIGIN_VALUE.to_string(),
        ),
    ]
    .into_iter()
    .collect();
    insert_run_objectives(&mut permission_metadata, run_context);
    crate::event_persistence::append_event(
        store,
        task_id,
        agent_core::EventKind::PermissionRequested,
        format!("Agent permission requested for {}", request.action),
        crate::project_session_persistence::metadata_with_context(permission_metadata, run_context),
    )
    .map_err(|error| error.to_string())?;
    Ok(request_id)
}

/// Read the durable decision for one pending subagent permission request.
fn subagent_permission_resolution(
    store: &SqliteStore,
    task_id: &TaskId,
    session_id: Option<&str>,
    agent_run_id: Option<&str>,
    request_id: &PermissionRequestId,
) -> Result<Option<PermissionResolution>, String> {
    let audits = match session_id {
        Some(session_id) => {
            store.list_permission_audits_for_session(task_id, session_id, agent_run_id, 0)
        }
        None => store.list_permission_audits(),
    }
    .map_err(|error| error.to_string())?;
    Ok(audits
        .into_iter()
        .find(|audit| audit.request.id == *request_id)
        .and_then(|audit| audit.resolution))
}

/// Park the subagent thread until the user resolves the patch approval or the
/// parent run stops. The run is NOT suspended: its tool batch is blocked on
/// this delegation, so the wait polls the durable store and honours
/// cancellation instead of using the run-level suspend/resume checkpoint
/// (which cannot capture a subagent's in-memory loop).
fn wait_for_subagent_permission(
    store: &Mutex<SqliteStore>,
    task_id: &TaskId,
    run_context: &Metadata,
    request_id: &PermissionRequestId,
    cancellation: &Arc<AgentRunControl>,
) -> Result<SubagentPermissionOutcome, String> {
    let session_id = run_context.get("session_id").map(String::as_str);
    let agent_run_id = run_context.get("agent_run_id").map(String::as_str);
    loop {
        if agent_run_should_stop(cancellation) {
            return Ok(SubagentPermissionOutcome::Cancelled);
        }
        let resolution = {
            let store = store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            subagent_permission_resolution(&store, task_id, session_id, agent_run_id, request_id)?
        };
        match resolution {
            Some(resolution) => {
                return Ok(match resolution.decision {
                    PermissionDecision::AllowOnce | PermissionDecision::AllowForSession => {
                        SubagentPermissionOutcome::Approved
                    }
                    PermissionDecision::Deny => SubagentPermissionOutcome::Denied,
                })
            }
            None => std::thread::sleep(SUBAGENT_PERMISSION_POLL_INTERVAL),
        }
    }
}

/// Execute one patch call of a write subagent: persist the parent-run
/// permission request, park until the user decides, then run the approved
/// patch through the same execution path the parent run uses (durable
/// started/finished events, shared tool budget, undo projection, workspace
/// cache invalidation). A denial becomes an ordinary denied observation, and
/// a cancellation before approval executes nothing.
fn execute_write_subagent_tool_call(
    context: &SubagentWriteContext<'_>,
    registry: &ToolRegistry,
    task_id: &TaskId,
    call: &agent_core::ModelToolCall,
    cancellation: &Arc<AgentRunControl>,
    description: &str,
) -> String {
    let invocation = ToolInvocation {
        id: ToolCallId(format!("subagent:{}:{}", context.parent_call_id, call.id)),
        task_id: task_id.clone(),
        tool_name: call.name.clone(),
        input_json: call.arguments_json.clone(),
        proposed_by_model: "subagent".to_string(),
        metadata: Metadata::new(),
    };
    let invocation_id = invocation.id.0.clone();
    let Some(tool) = registry.get(&call.name) else {
        return observation_from_tool_result(
            &call.name,
            "denied",
            "tool is not available to a subagent",
        );
    };
    let Some(request) = tool.permission_request(&invocation) else {
        // The patch tools always require approval; a missing request means the
        // call cannot be mediated, so it fails closed.
        return observation_from_tool_result(
            &call.name,
            "denied",
            "patch tool produced no permission request",
        );
    };
    let request_id = {
        let mut store = match context.state.store.lock() {
            Ok(store) => store,
            Err(error) => {
                return observation_from_tool_result(
                    &call.name,
                    "failed",
                    &format!("store lock poisoned: {error}"),
                )
            }
        };
        if let Err(error) = crate::tool_execution::append_tool_proposed_event(
            &mut store,
            &invocation,
            Some(context.run_context),
        )
        .map_err(|error| error.to_string())
        {
            return observation_from_tool_result(&call.name, "failed", &error);
        }
        match request_subagent_patch_approval(
            &mut store,
            task_id,
            context.run_context,
            &invocation,
            request,
            description,
        ) {
            Ok(request_id) => request_id,
            Err(error) => return observation_from_tool_result(&call.name, "failed", &error),
        }
    };
    let outcome = match wait_for_subagent_permission(
        &context.state.store,
        task_id,
        context.run_context,
        &request_id,
        cancellation,
    ) {
        Ok(outcome) => outcome,
        Err(error) => return observation_from_tool_result(&call.name, "failed", &error),
    };
    match outcome {
        SubagentPermissionOutcome::Cancelled => observation_from_tool_result(
            &call.name,
            "cancelled",
            "Subagent stopped before the patch was approved; no change was made.",
        ),
        SubagentPermissionOutcome::Denied => {
            let observation =
                observation_from_tool_result(&call.name, "denied", "The user denied this tool call.");
            if let Ok(mut store) = context.state.store.lock() {
                let _ = crate::tool_execution::append_tool_finished_event(
                    &mut store,
                    task_id,
                    &invocation_id,
                    &call.name,
                    "denied",
                    &observation,
                    [("failure_code".to_string(), "user_permission_denied".to_string())]
                        .into_iter()
                        .collect(),
                    Some(context.run_context),
                );
            }
            observation
        }
        SubagentPermissionOutcome::Approved => {
            match crate::tool_execution::execute_agent_tool_invocation_for_objective_epoch(
                context.state,
                registry,
                invocation,
                context.workspace_root,
                context.run_context,
                cancellation,
                agent_runtime::run_context_steer_epoch(context.run_context),
            ) {
                Ok(crate::tool_execution::AgentToolInvocationOutcome::Completed(result)) => {
                    observation_from_agent_tool_result(&call.name, &result)
                }
                Ok(crate::tool_execution::AgentToolInvocationOutcome::RestartAfterSteer) => {
                    observation_from_tool_result(
                        &call.name,
                        "cancelled",
                        "The approved patch was superseded by user steering before execution; no change was made.",
                    )
                }
                Err(error) => observation_from_tool_result(
                    &call.name,
                    "failed",
                    &format!("approved patch could not execute: {error}"),
                ),
            }
        }
    }
}

/// Execute one whitelisted read-only subagent tool call and format its
/// observation. The whitelist and the registry's read-only/permissionless guard
/// are both enforced, so a disallowed or effectful call becomes an observation
/// rather than an execution. Also used by the plan-mode drafting loop, which
/// shares the same read-only whitelist.
pub(crate) fn execute_subagent_tool_call(
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
pub(crate) fn strip_subagent_call_id(runtime: &mut agent_runtime::AgentLoopState, call_id: &str) {
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
#[path = "agent_subagent_runtime_tests.rs"]
mod tests;
