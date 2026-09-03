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
    modes: Mutex<Vec<ModelCallMode>>,
    first_messages: Mutex<Option<Vec<Message>>>,
    first_metadata: Mutex<Option<Metadata>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: AtomicUsize::new(0),
            modes: Mutex::new(Vec::new()),
            first_messages: Mutex::new(None),
            first_metadata: Mutex::new(None),
        }
    }

    fn served(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn requested_modes(&self) -> Vec<ModelCallMode> {
        self.modes.lock().expect("modes poisoned").clone()
    }

    /// The message list of the first model call, once captured.
    fn first_messages(&self) -> Option<Vec<Message>> {
        self.first_messages
            .lock()
            .expect("first messages poisoned")
            .clone()
    }

    /// The request metadata of the first model call, once captured.
    fn first_metadata(&self) -> Option<Metadata> {
        self.first_metadata
            .lock()
            .expect("first metadata poisoned")
            .clone()
    }
}

impl StreamingModelProvider for ScriptedProvider {
    fn complete_streaming_cancellable(
        &self,
        request: ModelRequest,
        _on_delta: &mut dyn FnMut(&str),
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<ModelResponse, model_provider::ModelError> {
        if should_cancel() {
            return Err(model_provider::ModelError::new("cancelled"));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.modes
            .lock()
            .expect("modes poisoned")
            .push(request.mode.clone());
        {
            let mut first_messages = self.first_messages.lock().expect("first messages poisoned");
            if first_messages.is_none() {
                *first_messages = Some(request.messages.clone());
            }
        }
        {
            let mut first_metadata = self.first_metadata.lock().expect("first metadata poisoned");
            if first_metadata.is_none() {
                *first_metadata = Some(request.metadata.clone());
            }
        }
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
    let tools = subagent_tool_specs_for_mode(&registry, false);

    let (_description, answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &[],
        "default",
    );

    assert_eq!(answer, "README.md:1 says line one");
    assert_eq!(provider.served(), 2);
}

#[test]
fn subagent_child_request_carries_the_parent_effort_reasoning() {
    let (_workspace, registry, task_id) = fixture();
    let provider = ScriptedProvider::new(vec![final_answer("done")]);
    let control = Arc::new(AgentRunControl::new("high"));
    let tools = subagent_tool_specs_for_mode(&registry, false);

    let (_description, _answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &[],
        "high",
    );

    // The child's model request carries the parent tier's reasoning effort so
    // thinking-family providers apply the tier's thinking budget (audit: children
    // previously ran with empty request metadata, so High/Xhigh never reached them).
    let metadata = provider.first_metadata().expect("first request metadata");
    assert_eq!(
        metadata
            .get(agent_core::REASONING_EFFORT_KEY)
            .map(String::as_str),
        Some("high")
    );
}

#[test]
fn subagent_child_requests_streaming_mode() {
    let (_workspace, registry, task_id) = fixture();
    let provider = ScriptedProvider::new(vec![
        tool_call_step(read_call("c1", "README.md")),
        final_answer("README.md:1 says line one"),
    ]);
    let control = Arc::new(AgentRunControl::new("fast"));
    let tools = subagent_tool_specs_for_mode(&registry, false);

    let (_description, answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &[],
        "default",
    );

    assert_eq!(answer, "README.md:1 says line one");
    assert_eq!(
        provider.requested_modes(),
        vec![ModelCallMode::Streaming, ModelCallMode::Streaming],
        "every child model call must declare the streaming mode it dispatches through"
    );
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
    let tools = subagent_tool_specs_for_mode(&registry, false);

    let (_description, answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &[],
        "default",
    );

    // The loop never exceeds the step budget even with an infinite tool-call
    // stream, and the answer carries the step-limit marker.
    assert!(provider.served() <= SUBAGENT_MAX_STEPS);
    assert!(answer.contains("step limit") || answer.contains("budget"));
}

#[test]
fn subagent_read_tool_executes_and_returns_line_addressable_content() {
    let (_workspace, registry, task_id) = fixture();
    let observation = execute_subagent_tool_call(
        &registry,
        &task_id,
        &read_call("r1", "README.md"),
        None,
        &Arc::new(AgentRunControl::new("fast")),
    );
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
    let observation = execute_subagent_tool_call(
        &registry,
        &task_id,
        &write_call,
        None,
        &Arc::new(AgentRunControl::new("fast")),
    );
    assert!(observation.contains("status=denied"));
    // The effectful tool never ran.
    assert!(!_workspace.path().join("evil.txt").exists());
}

#[test]
fn subagent_read_tool_is_gated_by_steer_and_cancel_before_execution() {
    let (_workspace, registry, task_id) = fixture();
    let control = Arc::new(AgentRunControl::new("fast"));
    control.request_cancel();
    let observation = execute_subagent_tool_call(
        &registry,
        &task_id,
        &read_call("r1", "README.md"),
        None,
        &control,
    );
    // The epoch/cancel gate refuses the call once the run is cancelled, so the
    // read never executes stale (audit P1-01 safe increment).
    assert!(
        observation.contains("superseded by user steering or cancellation"),
        "observation: {observation}"
    );
    assert!(!observation.contains("status=succeeded"));
}

#[test]
fn subagent_aborts_before_any_model_call_when_cancelled() {
    let (_workspace, registry, task_id) = fixture();
    let provider = ScriptedProvider::new(vec![final_answer("should not run")]);
    let control = Arc::new(AgentRunControl::new("fast"));
    control.request_cancel();
    let tools = subagent_tool_specs_for_mode(&registry, false);

    let (_description, answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &[],
        "default",
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
    let tools = subagent_tool_specs_for_mode(&registry, false);
    let (_description, answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &[],
        "default",
    );

    assert!(answer.contains("budget"));
    assert_eq!(provider.served(), 0);
}

#[test]
fn subagent_tool_surface_is_the_read_only_whitelist() {
    let (_workspace, registry, _task_id) = fixture();
    let names: Vec<String> = subagent_tool_specs_for_mode(&registry, false)
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(names.iter().any(|name| name == "file.read"));
    assert!(names.iter().any(|name| name == "file.glob"));
    assert!(names.iter().any(|name| name == "web.fetch"));
    assert!(!names.iter().any(|name| name == "file.write"));
    assert!(!names.iter().any(|name| name == "shell.run"));
}

fn parent_user(content: &str) -> Message {
    Message {
        role: MessageRole::User,
        content: content.to_string(),
        metadata: Metadata::new(),
    }
}

fn parent_assistant_text(content: &str) -> Message {
    Message {
        role: MessageRole::Assistant,
        content: content.to_string(),
        metadata: Metadata::new(),
    }
}

fn parent_assistant_call(call_id: &str) -> Message {
    let mut assistant = parent_assistant_text("");
    assistant.metadata.insert(
        "raw_tool_calls_json".to_string(),
        format!("[{{\"id\":\"{call_id}\"}}]"),
    );
    assistant
}

fn parent_tool_result(call_id: &str) -> Message {
    let mut result = Message {
        role: MessageRole::Tool,
        content: "observation".to_string(),
        metadata: Metadata::new(),
    };
    result
        .metadata
        .insert("tool_call_id".to_string(), call_id.to_string());
    result
}

/// Mirror of the fork `execute_subagent_delegations` performs before spawning
/// children.
fn fork_parent_context(messages: &[Message]) -> Vec<Message> {
    agent_runtime::subagent_context_fork_prefix(
        messages,
        agent_runtime::SUBAGENT_CONTEXT_FORK_MAX_MESSAGES,
    )
}

#[test]
fn subagent_fork_seeds_the_balanced_parent_prefix_before_the_contract() {
    let (_workspace, registry, task_id) = fixture();
    let parent = vec![
        parent_user("parent goal"),
        parent_assistant_call("p1"),
        parent_tool_result("p1"),
        parent_assistant_text("parent completed finding"),
    ];
    let prefix = fork_parent_context(&parent);
    assert_eq!(prefix.len(), parent.len());

    let provider = ScriptedProvider::new(vec![final_answer("done")]);
    let control = Arc::new(AgentRunControl::new("fast"));
    let tools = subagent_tool_specs_for_mode(&registry, false);
    let (_description, answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &prefix,
        "default",
    );

    assert_eq!(answer, "done");
    let messages = provider.first_messages().expect("first call captured");
    // The parent prefix leads, followed by the subagent system contract and
    // the delegated prompt.
    assert_eq!(messages.len(), prefix.len() + 2);
    assert_eq!(&messages[..prefix.len()], &prefix[..]);
    assert_eq!(messages[prefix.len()].role, MessageRole::System);
    assert_eq!(messages[prefix.len() + 1].role, MessageRole::User);
    assert!(messages[prefix.len() + 1]
        .content
        .contains("inspect readme"));
    assert!(
        agent_runtime::is_balanced_cut(&messages[..prefix.len()]),
        "the seeded prefix stays balanced"
    );
}

#[test]
fn subagent_fork_excludes_an_in_flight_parent_round() {
    let (_workspace, registry, task_id) = fixture();
    let mut parent = vec![
        parent_user("parent goal"),
        parent_assistant_call("p1"),
        parent_tool_result("p1"),
        parent_assistant_text("parent completed finding"),
    ];
    // The parent is mid-batch: this round's call has no result yet.
    parent.push(parent_user("follow-up"));
    parent.push(parent_assistant_call("p2"));

    let prefix = fork_parent_context(&parent);

    assert_eq!(prefix, &parent[..4], "the in-flight round is excluded");
    let provider = ScriptedProvider::new(vec![final_answer("done")]);
    let control = Arc::new(AgentRunControl::new("fast"));
    let tools = subagent_tool_specs_for_mode(&registry, false);
    let (_description, _answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &prefix,
        "default",
    );

    let messages = provider.first_messages().expect("first call captured");
    assert!(
        messages.iter().all(|message| !message
            .metadata
            .get("raw_tool_calls_json")
            .map(|raw| raw.contains("p2"))
            .unwrap_or(false)),
        "no in-flight parent call reaches the child"
    );
}

#[test]
fn subagent_fork_caps_prefix_messages_and_stays_balanced() {
    // Forty-message parent: ten completed rounds of four messages each.
    let mut parent = Vec::new();
    for round in 0..10 {
        parent.push(parent_user(&format!("request {round}")));
        parent.push(parent_assistant_call(&format!("p{round}")));
        parent.push(parent_tool_result(&format!("p{round}")));
        parent.push(parent_assistant_text(&format!("answer {round}")));
    }
    assert_eq!(parent.len(), 40);

    let prefix = fork_parent_context(&parent);

    assert!(prefix.len() <= agent_runtime::SUBAGENT_CONTEXT_FORK_MAX_MESSAGES);
    assert!(agent_runtime::is_balanced_cut(&prefix));
    assert_eq!(prefix.last(), parent.last());
}

#[test]
fn subagent_fork_without_parent_history_keeps_the_isolated_shape() {
    let (_workspace, registry, task_id) = fixture();
    let prefix = fork_parent_context(&[]);
    assert!(prefix.is_empty());

    let provider = ScriptedProvider::new(vec![final_answer("done")]);
    let control = Arc::new(AgentRunControl::new("fast"));
    let tools = subagent_tool_specs_for_mode(&registry, false);
    let (_description, _answer) = subagent_child_answer(
        &provider,
        &delegation_input(),
        &control,
        &registry,
        &tools,
        &task_id,
        None,
        None,
        &prefix,
        "default",
    );

    let messages = provider.first_messages().expect("first call captured");
    assert_eq!(
        messages.len(),
        2,
        "only the system contract and delegation prompt"
    );
    assert_eq!(messages[0].role, MessageRole::System);
    assert_eq!(messages[1].role, MessageRole::User);
}

fn write_run_context() -> Metadata {
    [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
        ("prompt_contract_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect()
}

fn patch_call(id: &str, path: &str) -> agent_core::ModelToolCall {
    agent_core::ModelToolCall {
        id: id.to_string(),
        name: "file.patch".to_string(),
        arguments_json: format!(
            r#"{{"path":"{path}","expected_base_sha256":"{}","anchor":"line two","replacement":"line 2"}}"#,
            "0".repeat(64)
        ),
    }
}

#[test]
fn allow_patches_gate_requires_request_and_effect_authority() {
    let mut context = write_run_context();
    assert_eq!(
        subagent_write_mode(&context, r#"{"description":"d"}"#),
        SubagentWriteMode::ReadOnly,
        "absent allow_patches keeps the delegation read-only"
    );
    assert_eq!(
        subagent_write_mode(&context, r#"{"description":"d","allow_patches":false}"#),
        SubagentWriteMode::ReadOnly
    );
    assert_eq!(
        subagent_write_mode(&context, r#"{"description":"d","allow_patches":"yes"}"#),
        SubagentWriteMode::ReadOnly,
        "a malformed flag never widens the surface"
    );
    // The default objective carries no effect constraint, so a requested
    // write delegation is admitted.
    assert_eq!(
        subagent_write_mode(&context, r#"{"description":"d","allow_patches":true}"#),
        SubagentWriteMode::Write
    );
    // A run whose objective forbids effects refuses the write surface even
    // when the model asked for it.
    context.insert(
        "effective_prompt_objective".to_string(),
        "Review the parser; do not change anything, read only.".to_string(),
    );
    assert_eq!(
        subagent_write_mode(&context, r#"{"description":"d","allow_patches":true}"#),
        SubagentWriteMode::Refused
    );
}

#[test]
fn write_subagent_surface_adds_exactly_the_patch_pair() {
    let (_workspace, registry, _task_id) = fixture();
    let names: Vec<String> = subagent_tool_specs_for_mode(&registry, true)
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(names.iter().any(|name| name == "file.read"));
    assert!(names.iter().any(|name| name == "file.patch"));
    assert!(names.iter().any(|name| name == "file.patch_batch"));
    for denied in [
        "file.write",
        "shell.run",
        "process.start",
        "todo.write",
        "task",
    ] {
        assert!(
            !names.iter().any(|name| name == denied),
            "{denied} must never enter the write-subagent surface"
        );
    }
}

#[test]
fn write_subagent_patch_request_carries_parent_identity_and_no_session_grant() {
    let (_workspace, registry, task_id) = fixture();
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = write_run_context();
    let invocation = ToolInvocation {
        id: ToolCallId("subagent:parent-call:p1".to_string()),
        task_id: task_id.clone(),
        tool_name: "file.patch".to_string(),
        input_json: patch_call("p1", "README.md").arguments_json,
        proposed_by_model: "subagent".to_string(),
        metadata: Metadata::new(),
    };
    let tool = registry.get("file.patch").expect("patch tool registered");
    let request = tool
        .permission_request(&invocation)
        .expect("patch requires permission");

    let request_id = request_subagent_patch_approval(
        &mut store,
        &task_id,
        &run_context,
        &invocation,
        request,
        "fix the readme",
    )
    .expect("request should persist");

    let pending = crate::permission_service::pending_agent_permissions_for_run(
        &store,
        &task_id,
        Some("session-a"),
        Some("run-a"),
    )
    .expect("pending requests should load");
    assert_eq!(pending.len(), 1);
    let pending = &pending[0];
    assert_eq!(pending.id, request_id);
    assert_eq!(pending.action, "file.patch");
    assert_eq!(pending.risk, agent_core::PermissionRisk::Write);
    assert_eq!(pending.scope, "README.md");
    assert_eq!(
        pending.metadata.get("session_id").map(String::as_str),
        Some("session-a"),
        "the request is raised in the parent run's name"
    );
    assert_eq!(
        pending
            .metadata
            .get(SUBAGENT_PERMISSION_ORIGIN_KEY)
            .map(String::as_str),
        Some("true")
    );
    assert!(
        !agent_core::permission_can_allow_session(pending),
        "a subagent patch request must never be session-approvable"
    );
    assert!(pending.reason.contains("Write subagent"));
    assert!(pending.reason.contains("fix the readme"));
    // The persisted request projects as a pending approval with no
    // session option.
    let audit = store
        .list_permission_audits()
        .expect("audits should load")
        .into_iter()
        .find(|audit| audit.request.id == request_id)
        .expect("audit record should exist");
    let view = crate::tool_execution::tool_approval_from_audit(audit)
        .expect("approval view should project");
    assert!(!view.can_allow_session);
    assert!(view.subagent);
}

#[test]
fn write_subagent_patch_never_reuses_a_parent_session_grant() {
    let (_workspace, registry, task_id) = fixture();
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = write_run_context();
    // The parent run already holds a session grant for this exact path.
    let mut granted = PermissionRequest {
        id: PermissionRequestId("parent-grant".to_string()),
        task_id: task_id.clone(),
        risk: agent_core::PermissionRisk::Write,
        action: "file.patch".to_string(),
        reason: "Patch README".to_string(),
        scope: "README.md".to_string(),
        metadata: Metadata::new(),
    };
    granted.metadata = merge_persistable_run_context(granted.metadata, &run_context);
    store
        .save_permission_request(granted.clone(), 1)
        .expect("grant request should save");
    store
        .resolve_permission(PermissionResolution {
            request_id: granted.id.clone(),
            decision: PermissionDecision::AllowForSession,
            resolved_at_ms: 2,
            resolved_by: "local-user".to_string(),
        })
        .expect("grant should resolve");
    assert!(crate::permission_service::agent_session_permission_granted(
        &store,
        &task_id,
        &granted,
        Some("session-a"),
    )
    .expect("grant check should run"));

    let invocation = ToolInvocation {
        id: ToolCallId("subagent:parent-call:p1".to_string()),
        task_id: task_id.clone(),
        tool_name: "file.patch".to_string(),
        input_json: patch_call("p1", "README.md").arguments_json,
        proposed_by_model: "subagent".to_string(),
        metadata: Metadata::new(),
    };
    let tool = registry.get("file.patch").expect("patch tool registered");
    let request = tool
        .permission_request(&invocation)
        .expect("patch requires permission");
    let request_id = request_subagent_patch_approval(
        &mut store,
        &task_id,
        &run_context,
        &invocation,
        request,
        "fix the readme",
    )
    .expect("request should persist");

    // The parent grant stays unused: the subagent request is pending and
    // must be resolved per call.
    let pending = crate::permission_service::pending_agent_permissions_for_run(
        &store,
        &task_id,
        Some("session-a"),
        Some("run-a"),
    )
    .expect("pending requests should load");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, request_id);
    assert_eq!(
        subagent_permission_resolution(
            &store,
            &task_id,
            Some("session-a"),
            Some("run-a"),
            &request_id,
        )
        .expect("resolution lookup should run"),
        None
    );
}

#[test]
fn write_subagent_observes_allow_once_and_deny_decisions() {
    let task_id = TaskId("subagent-test".to_string());
    let run_context = write_run_context();
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (index, decision) in [PermissionDecision::AllowOnce, PermissionDecision::Deny]
        .into_iter()
        .enumerate()
    {
        let mut request = PermissionRequest {
            id: PermissionRequestId(format!("agent-perm-{index}")),
            task_id: task_id.clone(),
            risk: agent_core::PermissionRisk::Write,
            action: "file.patch".to_string(),
            reason: "Patch README".to_string(),
            scope: "README.md".to_string(),
            metadata: Metadata::new(),
        };
        // Production requests carry the parent run identity through
        // `merge_persistable_run_context`; bind the same identity here so
        // the session-scoped audit query can find the record.
        request.metadata = merge_persistable_run_context(request.metadata, &run_context);
        let request_id = request.id.clone();
        store
            .save_permission_request(request, 10 + index as u64)
            .expect("request should save");
        assert_eq!(
            subagent_permission_resolution(
                &store,
                &task_id,
                Some("session-a"),
                Some("run-a"),
                &request_id,
            )
            .expect("lookup should run"),
            None,
            "an unresolved request keeps the subagent waiting"
        );
        store
            .resolve_permission(PermissionResolution {
                request_id: request_id.clone(),
                decision: decision.clone(),
                resolved_at_ms: 20 + index as u64,
                resolved_by: "local-user".to_string(),
            })
            .expect("resolution should save");
        let observed = subagent_permission_resolution(
            &store,
            &task_id,
            Some("session-a"),
            Some("run-a"),
            &request_id,
        )
        .expect("lookup should run")
        .expect("resolution should be visible");
        assert_eq!(observed.decision, decision);
    }
}

#[test]
fn write_subagent_wait_returns_cancelled_without_touching_the_store() {
    let task_id = TaskId("subagent-test".to_string());
    let store: Mutex<SqliteStore> =
        Mutex::new(SqliteStore::in_memory().expect("store should open"));
    let run_context = write_run_context();
    let control = Arc::new(AgentRunControl::new("fast"));
    control.request_cancel();
    let outcome = wait_for_subagent_permission(
        &store,
        &task_id,
        &run_context,
        &PermissionRequestId("agent-perm-never".to_string()),
        &control,
    )
    .expect("wait should return");
    assert_eq!(outcome, SubagentPermissionOutcome::Cancelled);
}

#[test]
fn write_subagent_wait_observes_an_allow_once_resolution() {
    let task_id = TaskId("subagent-test".to_string());
    let store: Mutex<SqliteStore> =
        Mutex::new(SqliteStore::in_memory().expect("store should open"));
    let run_context = write_run_context();
    {
        let mut guard = store.lock().expect("store lock");
        let mut request = PermissionRequest {
            id: PermissionRequestId("agent-perm-wait".to_string()),
            task_id: task_id.clone(),
            risk: agent_core::PermissionRisk::Write,
            action: "file.patch".to_string(),
            reason: "Patch README".to_string(),
            scope: "README.md".to_string(),
            metadata: Metadata::new(),
        };
        request.metadata = merge_persistable_run_context(request.metadata, &run_context);
        guard
            .save_permission_request(request, 10)
            .expect("request should save");
    }
    let waiter_store = &store;
    let control = Arc::new(AgentRunControl::new("fast"));
    let outcome = std::thread::scope(|scope| {
        let resolver = scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            let mut guard = waiter_store.lock().expect("store lock");
            guard
                .resolve_permission(PermissionResolution {
                    request_id: PermissionRequestId("agent-perm-wait".to_string()),
                    decision: PermissionDecision::AllowOnce,
                    resolved_at_ms: 20,
                    resolved_by: "local-user".to_string(),
                })
                .expect("resolution should save");
        });
        let outcome = wait_for_subagent_permission(
            &store,
            &task_id,
            &run_context,
            &PermissionRequestId("agent-perm-wait".to_string()),
            &control,
        )
        .expect("wait should return");
        resolver.join().expect("resolver should join");
        outcome
    });
    assert_eq!(outcome, SubagentPermissionOutcome::Approved);
}

#[test]
fn write_subagent_denial_never_executes_the_patch() {
    let (workspace, registry, task_id) = fixture();
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = write_run_context();
    let invocation = ToolInvocation {
        id: ToolCallId("subagent:parent-call:p1".to_string()),
        task_id: task_id.clone(),
        tool_name: "file.patch".to_string(),
        input_json: patch_call("p1", "README.md").arguments_json,
        proposed_by_model: "subagent".to_string(),
        metadata: Metadata::new(),
    };
    let request = registry
        .get("file.patch")
        .expect("patch tool registered")
        .permission_request(&invocation)
        .expect("patch requires permission");
    let request_id = request_subagent_patch_approval(
        &mut store,
        &task_id,
        &run_context,
        &invocation,
        request,
        "fix the readme",
    )
    .expect("request should persist");
    store
        .resolve_permission(PermissionResolution {
            request_id: request_id.clone(),
            decision: PermissionDecision::Deny,
            resolved_at_ms: 20,
            resolved_by: "local-user".to_string(),
        })
        .expect("resolution should save");
    let store = Mutex::new(store);
    let outcome = wait_for_subagent_permission(
        &store,
        &task_id,
        &run_context,
        &request_id,
        &Arc::new(AgentRunControl::new("fast")),
    )
    .expect("wait should return");
    assert_eq!(outcome, SubagentPermissionOutcome::Denied);
    // The denied patch never ran: the workspace file is untouched.
    let content =
        std::fs::read_to_string(workspace.path().join("README.md")).expect("README should exist");
    assert_eq!(content, "line one\nline two\n");
}

#[test]
fn every_subagent_allowlisted_tool_is_executable_through_a_worker_gate() {
    // Whitelist/capability drift guard (audit P2-01): the subagent surface
    // advertised web.search/web.fetch while the execution gate denied them.
    // Every name the surface advertises must pass one of the two worker
    // gates against a production-shaped registry, or the advertisement is a
    // lie and this test fails.
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let registry = ToolRegistry::with_workspace_tools_and_web_search(
        workspace.path(),
        tools::WebSearchConfig::default(),
    );
    let task_id = TaskId("subagent-allowlist".to_string());
    for name in agent_runtime::SUBAGENT_ALLOWED_TOOLS {
        let request = AgentToolRequest {
            call_id: agent_core::ToolCallId(format!("allowlist-{name}")),
            tool_name: (*name).to_string(),
            input: "{}".to_string(),
        };
        let invocation = tool_invocation_from_request(&task_id, &request);
        assert!(
            registry.permissionless_read_tool(&invocation).is_ok()
                || registry.worker_network_read_tool(&invocation).is_ok(),
            "{name} is advertised to subagents but no worker gate can execute it"
        );
    }
}

#[test]
fn subagent_network_tool_fails_closed_without_a_capability_context() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let registry = ToolRegistry::with_workspace_tools_and_web_search(
        workspace.path(),
        tools::WebSearchConfig::default(),
    );
    let task_id = TaskId("subagent-network-test".to_string());
    let web_call = ModelToolCall {
        id: "n1".to_string(),
        name: "web.fetch".to_string(),
        arguments_json: r#"{"url":"https://example.com/"}"#.to_string(),
    };
    let observation = execute_subagent_tool_call(
        &registry,
        &task_id,
        &web_call,
        None,
        &Arc::new(AgentRunControl::new("fast")),
    );
    assert!(
        observation.contains("status=failed") || observation.contains("status=denied"),
        "a network call without a capability context must never execute: {observation}"
    );
    assert!(
        observation.contains("network tools are not available"),
        "the observation must state the capability gap: {observation}"
    );
}
