use crate::agent_query_commands::{agent_run_should_stop, append_agent_progress_event};
use crate::app_state::AppState;
use crate::project_session_persistence::metadata_with_context;
use crate::runtime_values::phase16_task_id;
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
    PromptEffectAuthority, RunStageClass, SubagentChildOutcome, SubagentRunRecord,
    SubagentStopReason, SUBAGENT_MAX_STEPS, SUBAGENT_MAX_TOOL_CALLS,
};
use agent_storage::{PermissionStore, SqliteStore};
use model_provider::{ModelCallMode, ModelRequest, StreamingModelProvider};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tools::{Tool, ToolRegistry};

/// Metadata marker on a permission request created by a write subagent's
/// patch call. The permission resolver uses it to resolve the request in
/// place (the subagent thread is parked waiting on the decision) instead of
/// resuming a suspended run.
pub(crate) const SUBAGENT_PERMISSION_ORIGIN_KEY: &str = "subagent";
pub(crate) const SUBAGENT_PERMISSION_ORIGIN_VALUE: &str = "true";
const SUBAGENT_PERMISSION_POLL_INTERVAL: Duration = Duration::from_millis(150);
/// One owner for the description bound: the durable run record and a write
/// subagent's permission row must describe the same child with the same
/// truncation, so they can never disagree about which delegation is which.
const SUBAGENT_DESCRIPTION_MAX_CHARS: usize = agent_runtime::SUBAGENT_RECORD_DESCRIPTION_MAX_CHARS;

/// Split a tool-call batch, run any `task` delegations as isolated child
/// completions (returning their answers as internal instructions), and hand back
/// the remaining normal calls for regular execution. Delegations run concurrently
/// (preserving call order), honour run cancellation, and emit durable
/// started/finished progress events so the UI can show each subagent working.
/// A delegation may request `allow_patches`; the write surface is granted only
/// when the parent run's own prompt effect authority permits workspace effects.
///
/// Each delegation also leaves a durable [`SubagentRunRecord`] on its terminal
/// progress event, and its `subagent_result` message is persisted where it is
/// appended: both commit points downstream capture their `previous_message_count`
/// after this function appended it, so without persisting it here the child's
/// answer would exist only in memory and an app restart would replace it with a
/// synthetic "interrupted" tool observation.
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
    // Propagate the parent run's effort tier to child model calls so subagents
    // honor the tier's reasoning/thinking budget (audit: children previously ran
    // with empty request metadata, so High/Xhigh reasoning never reached them).
    let reasoning_effort = run_context
        .get("agent_effort")
        .cloned()
        .unwrap_or_else(|| "default".to_string());
    // The model the run's actor provider serves. Children share the parent's
    // provider today; the record states which one answered the delegation.
    let run_model = run_context.get("agent_model").cloned().unwrap_or_default();
    for (call, write_mode) in task_calls.iter().zip(&write_modes) {
        let description = subagent_description(&call.input);
        let summary = match write_mode {
            SubagentWriteMode::ReadOnly => format!("Subagent started: {description}"),
            SubagentWriteMode::Write => format!("Write subagent started: {description}"),
            SubagentWriteMode::Refused => {
                format!("Write subagent refused: {description} (run forbids workspace effects)")
            }
        };
        let mut event_context = run_context.clone();
        if matches!(write_mode, SubagentWriteMode::Refused) {
            // A refused delegation never starts a child, so this start event is
            // also its terminal one and carries the run record: no finish event
            // will ever be emitted for it.
            let record = SubagentRunRecord::new(
                call.call_id.0.clone(),
                &child_outcome(
                    &description,
                    subagent_write_refused_answer(),
                    SubagentStopReason::Refused,
                    0,
                    0,
                ),
                true,
                &run_model,
                &reasoning_effort,
            );
            record.insert_metadata(&mut event_context);
        }
        let _ = append_agent_progress_event(state, &runtime.task_id, &event_context, &summary);
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
    let outcomes: Vec<SubagentChildOutcome> = std::thread::scope(|scope| {
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
                        Some(SubagentNetworkContext { state, run_context }),
                        &context_prefix,
                        &reasoning_effort,
                    )
                }))
            })
            .collect();
        handles
            .into_iter()
            .zip(&task_calls)
            .map(|(handle, call)| match handle {
                Some(handle) => match handle.join() {
                    Ok(outcome) => outcome,
                    // A panicking child reports zero counters: its step and
                    // tool-call counts lived on the thread that unwound, so the
                    // record states the crash rather than inventing a total.
                    Err(_) => child_outcome(
                        "delegated task",
                        "Subagent did not complete.".to_string(),
                        SubagentStopReason::Crashed,
                        0,
                        0,
                    ),
                },
                None => child_outcome(
                    &subagent_description(&call.input),
                    subagent_write_refused_answer(),
                    SubagentStopReason::Refused,
                    0,
                    0,
                ),
            })
            .collect()
    });
    for ((call, write_mode), outcome) in task_calls.iter().zip(&write_modes).zip(outcomes) {
        strip_subagent_call_id(runtime, &call.call_id.0);
        agent_runtime::append_internal_instruction(
            runtime,
            "subagent_result",
            &format!(
                "Subagent result for {:?}:\n{}",
                outcome.description, outcome.answer
            ),
        );
        // Persist the appended result message where it is appended. It carries
        // `internal=true`, so the chat projection skips it while the recovery and
        // resume transcripts keep it: the delegated answer survives an app restart
        // without surfacing a raw internal instruction in the thread.
        if let Some(message) = runtime.messages.last() {
            persist_subagent_result_message(state, &runtime.task_id, run_context, message);
        }
        let summary = match write_mode {
            SubagentWriteMode::ReadOnly => {
                Some(format!("Subagent finished: {}", outcome.description))
            }
            SubagentWriteMode::Write => {
                Some(format!("Write subagent finished: {}", outcome.description))
            }
            // The refusal was already announced, and recorded, when the
            // delegation arrived.
            SubagentWriteMode::Refused => None,
        };
        if let Some(summary) = summary {
            let mut event_context = run_context.clone();
            let record = SubagentRunRecord::new(
                call.call_id.0.clone(),
                &outcome,
                matches!(write_mode, SubagentWriteMode::Write),
                &run_model,
                &reasoning_effort,
            );
            if !record.insert_metadata(&mut event_context) {
                eprintln!(
                    "subagent run record for {:?} was not valid and was not recorded",
                    outcome.description
                );
            }
            let _ = append_agent_progress_event(state, &runtime.task_id, &event_context, &summary);
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

/// Everything a child needs to mediate a read-semantics network call
/// (web.search/web.fetch) through the parent run's permission path. An
/// existing `allow_for_session` capability resolved under the parent session
/// lets the call run directly; otherwise a one-shot auditable approval
/// request is raised under the parent run identity, and an AllowForSession
/// decision becomes the inherited capability for later calls. Without this
/// context, network tools fail closed.
pub(crate) struct SubagentNetworkContext<'a> {
    pub(crate) state: &'a tauri::State<'a, AppState>,
    pub(crate) run_context: &'a Metadata,
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
    network: Option<SubagentNetworkContext<'_>>,
    parent_context: &[Message],
    reasoning_effort: &str,
) -> SubagentChildOutcome {
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
    let mut child_tool_calls = 0usize;
    // Model turns actually attempted, for the durable run record. A turn the
    // cancel check skipped is not counted: it did no work.
    let mut steps = 0usize;
    for _step in 0..SUBAGENT_MAX_STEPS {
        if agent_run_should_stop(cancellation) {
            return child_outcome(
                &description,
                subagent_stopped_answer(&last_content),
                SubagentStopReason::Cancelled,
                steps,
                child_tool_calls,
            );
        }
        steps += 1;
        // Charge the model call to the parent run's Worker stage budget; an
        // exhausted stage budget stops the child loop without stopping the run.
        if cancellation
            .begin_stage_model_call("subagent", RunStageClass::Worker)
            .is_err()
        {
            let reason = subagent_stage_stop_reason(cancellation);
            return child_outcome(
                &description,
                subagent_stop_answer(reason, &last_content),
                reason,
                steps,
                child_tool_calls,
            );
        }
        // Propagate the parent run's effort tier so the child's model call honors
        // the tier's reasoning/thinking budget instead of defaulting to empty
        // metadata (audit: children previously shared the actor provider with no
        // reasoning effort, so High/Xhigh never reached subagent calls).
        let mut request_metadata = Metadata::new();
        let effort = reasoning_effort.trim();
        if !effort.is_empty() {
            request_metadata.insert(
                agent_core::REASONING_EFFORT_KEY.to_string(),
                effort.to_string(),
            );
        }
        let request = ModelRequest {
            role: ModelRole::Executor,
            messages: messages.clone(),
            tools: subagent_tools.to_vec(),
            // The child dispatches through `complete_streaming_cancellable`, so
            // declare the streaming mode the transport actually runs.
            mode: ModelCallMode::Streaming,
            metadata: request_metadata,
        };
        let mut should_cancel = || agent_run_should_stop(cancellation);
        // Reserve a physical attempt on the unified ledger (audit P1-01): the
        // logical stage call above bounded the child loop, but the physical
        // tokens/attempts were never counted, so parallel children could
        // amplify provider calls for free under the parent budget.
        let outcome = crate::model_resource_runtime::controlled_aux_model_call(
            cancellation,
            "subagent",
            &request,
            RunStageClass::Worker,
            || {
                actor_provider.complete_streaming_cancellable(
                    request.clone(),
                    &mut |_| {},
                    &mut should_cancel,
                )
            },
        );
        cancellation.finish_model_call();
        let response = match outcome {
            crate::model_resource_runtime::AuxModelCall::Response(response) => response,
            // A refused physical attempt is either the Worker budget or a steer
            // that superseded this delegation. The two are distinguishable, and
            // reporting a steered child as a budget exhaustion tells the parent
            // something false about why the delegation ended.
            crate::model_resource_runtime::AuxModelCall::BudgetExhausted => {
                let reason = subagent_stage_stop_reason(cancellation);
                return child_outcome(
                    &description,
                    subagent_stop_answer(reason, &last_content),
                    reason,
                    steps,
                    child_tool_calls,
                );
            }
            crate::model_resource_runtime::AuxModelCall::Stopped
            | crate::model_resource_runtime::AuxModelCall::ProviderError => {
                let reason = if agent_run_should_stop(cancellation) {
                    SubagentStopReason::Cancelled
                } else if subagent_run_was_steered(cancellation) {
                    SubagentStopReason::Steered
                } else {
                    SubagentStopReason::ProviderUnavailable
                };
                return child_outcome(
                    &description,
                    if last_content.is_empty() {
                        "Subagent could not run (provider unavailable).".to_string()
                    } else {
                        last_content
                    },
                    reason,
                    steps,
                    child_tool_calls,
                );
            }
        };
        last_content = response.message.content.clone();
        if response.tool_calls.is_empty() {
            return child_outcome(
                &description,
                last_content,
                SubagentStopReason::Completed,
                steps,
                child_tool_calls,
            );
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
            // Per-child tool-call budget (P1-01 resource governance): stop the
            // child rather than amplify tool calls unbounded across parallel
            // children; exhaustion stops the child, not the parent run.
            if child_tool_calls >= SUBAGENT_MAX_TOOL_CALLS {
                return child_outcome(
                    &description,
                    subagent_budget_answer(&last_content),
                    SubagentStopReason::ToolCallBudget,
                    steps,
                    child_tool_calls,
                );
            }
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
                _ => execute_subagent_tool_call(
                    registry,
                    task_id,
                    call,
                    network.as_ref(),
                    cancellation,
                ),
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
            child_tool_calls += 1;
        }
    }
    child_outcome(
        &description,
        subagent_step_limit_answer(&last_content),
        SubagentStopReason::StepLimit,
        steps,
        child_tool_calls,
    )
}

/// The child loop's own report type lives in `agent_runtime::subagent` beside the
/// caps it reports against, so the durable record and the eval harness read the
/// same vocabulary; this helper only shortens the loop's seven stop sites.
fn child_outcome(
    description: &str,
    answer: String,
    stop_reason: SubagentStopReason,
    steps: usize,
    tool_calls: usize,
) -> SubagentChildOutcome {
    SubagentChildOutcome::new(description, answer, stop_reason, steps, tool_calls)
}

/// Whether the parent run's objective moved under this child: a steer is pending,
/// or the child can no longer act on the epoch it started with.
fn subagent_run_was_steered(cancellation: &Arc<AgentRunControl>) -> bool {
    cancellation.has_pending_steer()
        || !cancellation.objective_epoch_is_current(cancellation.steer_epoch())
}

/// Why a child's model-call reservation was refused. Cancellation wins, then a
/// steer that superseded the delegation, then the Worker stage budget.
fn subagent_stage_stop_reason(cancellation: &Arc<AgentRunControl>) -> SubagentStopReason {
    if agent_run_should_stop(cancellation) {
        SubagentStopReason::Cancelled
    } else if subagent_run_was_steered(cancellation) {
        SubagentStopReason::Steered
    } else {
        SubagentStopReason::StageBudget
    }
}

/// The answer text for a child that stopped before finishing. Each reason keeps
/// the wording the parent already receives for it, except `Steered`, which used
/// to be indistinguishable from a stage-budget exhaustion.
fn subagent_stop_answer(reason: SubagentStopReason, partial: &str) -> String {
    match reason {
        SubagentStopReason::Steered => subagent_steered_answer(partial),
        SubagentStopReason::Cancelled => subagent_stopped_answer(partial),
        SubagentStopReason::StepLimit => subagent_step_limit_answer(partial),
        _ => subagent_budget_answer(partial),
    }
}

fn subagent_steered_answer(partial: &str) -> String {
    if partial.trim().is_empty() {
        "Subagent stopped: the run was steered before this delegation produced an answer."
            .to_string()
    } else {
        format!("{partial}\n\n[subagent stopped: superseded by user steering]")
    }
}

/// Persist a delegation's `subagent_result` message as a durable `MessageAdded`
/// event, with exactly the metadata the in-memory message carries plus the run
/// context every persisted message carries.
///
/// Best-effort like the progress events: a store failure must not lose the
/// in-memory result the parent is about to reason over.
fn persist_subagent_result_message(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    message: &Message,
) {
    let persisted = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))
        .and_then(|mut store| {
            crate::event_persistence::append_message_event_with_metadata(
                &mut store,
                task_id,
                message.role.clone(),
                &message.content,
                metadata_with_context(message.metadata.clone(), run_context),
            )
            .map_err(|error| format!("failed to persist the subagent result message: {error}"))
        });
    if let Err(error) = persisted {
        eprintln!("{error}");
    }
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
    request: PermissionRequest,
    description: &str,
) -> Result<PermissionRequestId, String> {
    let bounded_description: String = description
        .chars()
        .take(SUBAGENT_DESCRIPTION_MAX_CHARS)
        .collect();
    let mut request = request;
    request.metadata.insert(
        "subagent_description".to_string(),
        bounded_description.clone(),
    );
    // Patch calls are one-shot: session grants never apply to them.
    request_subagent_mediated_approval(
        store,
        task_id,
        run_context,
        invocation,
        request,
        format!("Write subagent \"{bounded_description}\" requests approval"),
        false,
    )
}

/// Raise the auditable approval request for one subagent network call. The
/// request is session-reusable so an AllowForSession decision persists as the
/// inherited capability that later subagent network calls check first.
fn request_subagent_network_approval(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    invocation: &ToolInvocation,
    request: PermissionRequest,
) -> Result<PermissionRequestId, String> {
    request_subagent_mediated_approval(
        store,
        task_id,
        run_context,
        invocation,
        request,
        "Subagent requests public-web network access".to_string(),
        true,
    )
}

/// Shared durable core of the subagent-mediated approval path: persist the
/// permission request under the parent run identity (subagent origin marker,
/// run context, objectives) and append the PermissionRequested event. The
/// caller chooses the reason prefix and whether the decision may become a
/// reusable session capability.
fn request_subagent_mediated_approval(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    invocation: &ToolInvocation,
    mut request: PermissionRequest,
    reason_prefix: String,
    session_reusable: bool,
) -> Result<PermissionRequestId, String> {
    request.reason = format!("{reason_prefix}: {}", request.reason);
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
        .insert("session_reusable".to_string(), session_reusable.to_string());
    request.metadata.insert(
        SUBAGENT_PERMISSION_ORIGIN_KEY.to_string(),
        SUBAGENT_PERMISSION_ORIGIN_VALUE.to_string(),
    );
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
/// rather than an execution. Network reads (web.search/web.fetch) are a
/// separate named capability: they run only when the parent session already
/// holds an `allow_for_session` network grant, or after a one-shot auditable
/// approval raised under the parent run identity. Without a
/// `SubagentNetworkContext` (for example the plan-drafting loop) network
/// tools fail closed. Also used by the plan-mode drafting loop, which shares
/// the same read-only whitelist.
pub(crate) fn execute_subagent_tool_call(
    registry: &ToolRegistry,
    task_id: &TaskId,
    call: &agent_core::ModelToolCall,
    network: Option<&SubagentNetworkContext>,
    cancellation: &Arc<AgentRunControl>,
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
    // Epoch/cancel gate before any subagent tool dispatch (audit P1-01, safe
    // increment): a steer or cancel that lands while the child loop is running
    // must stop the call rather than execute stale discovery. This extends the
    // network-path cancel re-check to read tools and adds the objective-epoch
    // check. It intentionally does NOT consume the parent tool-call budget:
    // coupling subagent reads to the run-level tool budget makes exhaustion set a
    // run-wide stop_reason and breaks the child loop's bounded-by-steps contract,
    // so unifying the tool budget needs a subagent-scoped accounting design and
    // is deferred rather than rushed.
    let tool_epoch = cancellation.steer_epoch();
    if agent_run_should_stop(cancellation) || !cancellation.objective_epoch_is_current(tool_epoch) {
        return observation_from_tool_result(
            &call.name,
            "cancelled",
            "The subagent tool call was superseded by user steering or cancellation before execution; no action was taken.",
        );
    }
    // Read-only discovery gate first; the controlled network capability
    // (web.search/web.fetch) is a separate named-allowlist gate wrapped in
    // the parent-run permission path, so a denial from the pure-read gate is
    // not a denial of the network reads the subagent surface promises.
    let result = match registry.permissionless_read_tool(&invocation) {
        Ok(tool) => {
            // Durable started event for the subagent read (P1-01): the read path
            // previously executed without durable started/finished events, so
            // child discovery was invisible to the run's durable tool lineage.
            if let Some(context) = network {
                if let Ok(mut store) = context.state.store.lock() {
                    let _ = crate::tool_execution::append_tool_proposed_event(
                        &mut store,
                        &invocation,
                        Some(context.run_context),
                    );
                }
            }
            let result = tool.execute(invocation).unwrap_or_else(|error| {
                agent_core::ToolResult::failed(
                    agent_core::ToolCallId(call.id.clone()),
                    error.message,
                )
            });
            if let Some(context) = network {
                if let Ok(mut store) = context.state.store.lock() {
                    let status = match result.status {
                        ToolOutcomeStatus::Succeeded => "succeeded",
                        ToolOutcomeStatus::Failed => "failed",
                        ToolOutcomeStatus::Cancelled => "cancelled",
                        ToolOutcomeStatus::Denied => "denied",
                    };
                    let _ = crate::tool_execution::append_tool_finished_event(
                        &mut store,
                        task_id,
                        &call.id,
                        &call.name,
                        status,
                        &result.output,
                        Metadata::new(),
                        Some(context.run_context),
                    );
                }
            }
            result
        }
        Err(_) => match registry.worker_network_read_tool(&invocation) {
            Ok(tool) => execute_worker_network_tool(
                tool,
                invocation,
                &call.id,
                task_id,
                network,
                cancellation,
            ),
            Err(error) => agent_core::ToolResult::failed(
                agent_core::ToolCallId(call.id.clone()),
                error.message,
            ),
        },
    };
    // Aggregate the child's tool call into the parent run's tool-call accounting
    // (P1-01) so the parent budget/snapshot reflect real child usage; the child
    // itself is bounded by its per-child cap and the epoch/cancel gate above.
    cancellation.record_external_tool_call();
    let status = match result.status {
        ToolOutcomeStatus::Succeeded => "succeeded",
        ToolOutcomeStatus::Failed => "failed",
        ToolOutcomeStatus::Cancelled => "cancelled",
        ToolOutcomeStatus::Denied => "denied",
    };
    observation_from_tool_result(&call.name, status, &result.output)
}

/// Mediate one read-semantics network call through the parent run's
/// permission path. An inherited `allow_for_session` capability executes the
/// call directly; otherwise a one-shot auditable approval request is raised
/// under the parent run identity and the call parks on the user's decision
/// (AllowForSession becomes the inherited capability for later calls). Every
/// path that cannot prove an explicit grant fails closed.
fn execute_worker_network_tool(
    tool: &dyn Tool,
    invocation: ToolInvocation,
    call_id: &str,
    task_id: &TaskId,
    network: Option<&SubagentNetworkContext>,
    cancellation: &Arc<AgentRunControl>,
) -> agent_core::ToolResult {
    let fail = |message: String| {
        agent_core::ToolResult::failed(agent_core::ToolCallId(call_id.to_string()), message)
    };
    let Some(context) = network else {
        return fail(
            "network tools are not available in this isolated context; ask the parent run to fetch the page instead".to_string(),
        );
    };
    let Some(request) = tool.permission_request(&invocation) else {
        return fail("network tool produced no permission request".to_string());
    };
    // Inherited capability: an allow_for_session decision already resolved
    // under the parent session covers this exact tool action.
    let granted = {
        let store = match context.state.store.lock() {
            Ok(store) => store,
            Err(error) => return fail(format!("store lock poisoned: {error}")),
        };
        let session_id = context.run_context.get("session_id").map(String::as_str);
        match crate::permission_service::agent_session_permission_granted(
            &store,
            &phase16_task_id(),
            &request,
            session_id,
        ) {
            Ok(granted) => granted,
            Err(error) => return fail(format!("session capability check failed: {error}")),
        }
    };
    if !granted {
        let request_id = {
            let mut store = match context.state.store.lock() {
                Ok(store) => store,
                Err(error) => return fail(format!("store lock poisoned: {error}")),
            };
            if let Err(error) = crate::tool_execution::append_tool_proposed_event(
                &mut store,
                &invocation,
                Some(context.run_context),
            )
            .map_err(|error| error.to_string())
            {
                return fail(error);
            }
            match request_subagent_network_approval(
                &mut store,
                task_id,
                context.run_context,
                &invocation,
                request,
            ) {
                Ok(request_id) => request_id,
                Err(error) => return fail(error),
            }
        };
        match wait_for_subagent_permission(
            &context.state.store,
            task_id,
            context.run_context,
            &request_id,
            cancellation,
        ) {
            Ok(SubagentPermissionOutcome::Approved) => {}
            Ok(SubagentPermissionOutcome::Denied) => {
                return agent_core::ToolResult::text(
                    agent_core::ToolCallId(call_id.to_string()),
                    ToolOutcomeStatus::Denied,
                    "The user denied this network request.",
                    Metadata::new(),
                );
            }
            Ok(SubagentPermissionOutcome::Cancelled) => {
                return fail(
                    "subagent stopped before the network request was approved".to_string(),
                );
            }
            Err(error) => return fail(error),
        }
    }
    // Re-check cancellation/steer immediately before the outbound call: the
    // approval above may have parked on user input, and a cancel or steer during
    // that window must prevent the network egress (audit P1-04). The inherited
    // capability path is cheap to re-check too, so this covers both.
    if crate::agent_query_commands::agent_run_should_stop(cancellation) {
        return fail("subagent stopped before the approved network request executed".to_string());
    }
    tool.execute(invocation).unwrap_or_else(|error| {
        agent_core::ToolResult::failed(agent_core::ToolCallId(call_id.to_string()), error.message)
    })
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
