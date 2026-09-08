use super::*;
use agent_core::{EventId, ModelToolCall};
use agent_storage::SqliteStore;
use model_provider::ModelResponse;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A scripted provider that replays a fixed queue of responses and counts how
/// many model calls it served, so tests can drive a deterministic plan-drafting
/// loop without a network (mirrors the subagent loop tests).
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
    (workspace, registry, TaskId("plan-mode-test".to_string()))
}

fn plan_text() -> &'static str {
    "1. Inspect src/lib.rs\n2. Add the flag\n\nFiles: src/lib.rs"
}

fn run_context(plan_requested: bool) -> Metadata {
    let mut context = Metadata::from([
        ("session_id".to_string(), "session-plan".to_string()),
        ("agent_run_id".to_string(), "run-plan".to_string()),
        ("logical_agent_run_id".to_string(), "run-plan".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]);
    if plan_requested {
        context.insert(PLAN_MODE_REQUESTED_KEY.to_string(), "true".to_string());
    }
    context
}

fn event(sequence: u64, summary: &str, metadata: Metadata) -> Event {
    Event {
        id: EventId(format!("event-{sequence}")),
        task_id: TaskId("agent".to_string()),
        sequence,
        timestamp_ms: 1_000 + sequence,
        kind: EventKind::TaskStatusChanged,
        summary: summary.to_string(),
        metadata,
    }
}

fn start_event(sequence: u64, plan_requested: bool) -> Event {
    event(sequence, "Agent task started", run_context(plan_requested))
}

fn proposed_event(sequence: u64, plan_markdown: &str) -> Event {
    let mut metadata = plan_proposed_event_metadata(plan_markdown);
    metadata.extend(run_context(true));
    event(sequence, PLAN_PROPOSED_SUMMARY, metadata)
}

fn paused_event(sequence: u64) -> Event {
    event(sequence, "Agent task paused", run_context(true))
}

fn resolved_event(sequence: u64, plan_markdown: &str, decision: PlanConfirmationDecision) -> Event {
    let mut metadata =
        plan_resolved_event_metadata(decision, &plan_digest(plan_markdown), PLAN_RESOLVED_BY_USER);
    metadata.extend(run_context(true));
    event(sequence, PLAN_RESOLVED_SUMMARY, metadata)
}

#[test]
fn plan_resolution_source_is_parsed_fail_closed_and_audited() {
    assert_eq!(
        parse_plan_resolved_by("local-user").unwrap(),
        PLAN_RESOLVED_BY_USER
    );
    assert_eq!(parse_plan_resolved_by("").unwrap(), PLAN_RESOLVED_BY_USER);
    assert_eq!(
        parse_plan_resolved_by("auto-timeout").unwrap(),
        PLAN_RESOLVED_BY_AUTO_TIMEOUT
    );
    assert!(parse_plan_resolved_by("guardian").is_err());
    assert!(parse_plan_resolved_by("timer").is_err());

    let metadata = plan_resolved_event_metadata(
        PlanConfirmationDecision::Approved,
        "digest",
        PLAN_RESOLVED_BY_AUTO_TIMEOUT,
    );
    assert_eq!(
        metadata.get("plan_resolved_by").map(String::as_str),
        Some(PLAN_RESOLVED_BY_AUTO_TIMEOUT),
        "timeout approvals must be distinguishable from user clicks in the audit trail"
    );
}

#[test]
fn plan_mode_contract_gate_is_limited_to_high_and_xhigh() {
    assert!(!plan_mode_available_for_effort(AgentPolicy::Fast));
    assert!(!plan_mode_available_for_effort(AgentPolicy::Default));
    assert!(plan_mode_available_for_effort(AgentPolicy::High));
    assert!(plan_mode_available_for_effort(AgentPolicy::Xhigh));
}

#[test]
fn plan_mode_contract_request_is_dropped_for_fast_and_default() {
    assert!(!plan_mode_gate_active(true, AgentPolicy::Fast));
    assert!(!plan_mode_gate_active(true, AgentPolicy::Default));
    assert!(!plan_mode_gate_active(false, AgentPolicy::High));
    assert!(plan_mode_gate_active(true, AgentPolicy::High));
    assert!(plan_mode_gate_active(true, AgentPolicy::Xhigh));
}

#[test]
fn plan_mode_contract_request_flag_reads_from_the_start_event() {
    let events = vec![start_event(1, true), proposed_event(2, plan_text())];
    assert!(plan_mode_requested_in_events(&events));
    let plain = vec![start_event(1, false)];
    assert!(!plan_mode_requested_in_events(&plain));
    let mut context = run_context(true);
    assert!(plan_mode_requested_in_context(&context));
    context.remove(PLAN_MODE_REQUESTED_KEY);
    assert!(!plan_mode_requested_in_context(&context));
}

#[test]
fn plan_mode_contract_loop_runs_read_only_tools_to_a_plan() {
    let (_workspace, registry, task_id) = fixture();
    let provider = ScriptedProvider::new(vec![
        tool_call_step(read_call("c1", "README.md")),
        final_answer(plan_text()),
    ]);
    let control = Arc::new(AgentRunControl::new("high"));
    let tools = plan_phase_tool_specs(&registry);

    let outcome = run_plan_phase(
        &provider,
        "document the crate",
        &control,
        &registry,
        &tools,
        &task_id,
        &mut |_, _, _| {},
    );

    assert_eq!(outcome, PlanPhaseOutcome::Proposed(plan_text().to_string()));
    assert_eq!(provider.served(), 2);
}

/// Review F2: the plan-drafting loop shares the read-only dispatch entry,
/// and providers echo wire-form names there too. A raw `file_read` must
/// execute (not be denied) and surface under its canonical name.
#[test]
fn plan_mode_contract_loop_normalizes_provider_wire_tool_names() {
    let (_workspace, registry, task_id) = fixture();
    let provider = ScriptedProvider::new(vec![
        tool_call_step(ModelToolCall {
            id: "c1".to_string(),
            name: "file_read".to_string(),
            arguments_json: r#"{"path":"README.md"}"#.to_string(),
        }),
        final_answer(plan_text()),
    ]);
    let control = Arc::new(AgentRunControl::new("high"));
    let tools = plan_phase_tool_specs(&registry);

    let mut seen = Vec::new();
    let outcome = run_plan_phase(
        &provider,
        "document the crate",
        &control,
        &registry,
        &tools,
        &task_id,
        &mut |name: &str, _id: &str, observation: &str| {
            seen.push((
                name.to_string(),
                observation.lines().nth(1).unwrap_or("").to_string(),
            ));
        },
    );

    assert!(matches!(outcome, PlanPhaseOutcome::Proposed(_)));
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "file.read", "the report carries the canonical name");
    assert_eq!(
        seen[0].1, "status=succeeded",
        "the wire-name exploration read must execute, not be denied"
    );
}

#[test]
fn plan_mode_contract_loop_denies_effectful_tools() {
    let (workspace, registry, task_id) = fixture();
    let write_call = ModelToolCall {
        id: "w1".to_string(),
        name: "file.write".to_string(),
        arguments_json: r#"{"path":"evil.txt","content":"x"}"#.to_string(),
    };
    let provider =
        ScriptedProvider::new(vec![tool_call_step(write_call), final_answer(plan_text())]);
    let control = Arc::new(AgentRunControl::new("high"));
    let tools = plan_phase_tool_specs(&registry);

    let outcome = run_plan_phase(
        &provider,
        "write a file",
        &control,
        &registry,
        &tools,
        &task_id,
        &mut |_, _, _| {},
    );

    // The effectful call became a denied observation, the loop continued, and
    // the write never executed.
    assert!(matches!(outcome, PlanPhaseOutcome::Proposed(_)));
    assert!(!workspace.path().join("evil.txt").exists());
    assert_eq!(provider.served(), 2);
}

#[test]
fn plan_mode_contract_tool_surface_is_the_read_only_whitelist() {
    let (_workspace, registry, _task_id) = fixture();
    let names: Vec<String> = plan_phase_tool_specs(&registry)
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(names.iter().any(|name| name == "file.read"));
    assert!(names.iter().any(|name| name == "file.list"));
    assert!(names.iter().any(|name| name == "file.search"));
    assert!(names.iter().any(|name| name == "file.glob"));
    assert!(names.iter().any(|name| name == "web.fetch"));
    assert!(!names.iter().any(|name| name == "file.write"));
    assert!(!names.iter().any(|name| name == "shell.run"));
}

#[test]
fn plan_mode_contract_model_calls_charge_the_worker_stage_budget() {
    let (_workspace, registry, task_id) = fixture();
    let control = Arc::new(AgentRunControl::new("high"));
    // Exhaust the Worker stage budget so the plan loop's first charged call
    // must fail closed.
    let mut exhausted = false;
    for _ in 0..600 {
        if control
            .begin_stage_model_call("subagent", RunStageClass::Worker)
            .is_err()
        {
            exhausted = true;
            break;
        }
    }
    assert!(exhausted, "worker stage budget should be bounded");

    let provider = ScriptedProvider::new(vec![final_answer(plan_text())]);
    let tools = plan_phase_tool_specs(&registry);
    let outcome = run_plan_phase(
        &provider,
        "plan it",
        &control,
        &registry,
        &tools,
        &task_id,
        &mut |_, _, _| {},
    );

    assert!(matches!(
        outcome,
        PlanPhaseOutcome::Unavailable(ref reason) if reason.contains("budget")
    ));
    assert_eq!(provider.served(), 0);
}

#[test]
fn plan_mode_contract_loop_aborts_before_any_model_call_when_cancelled() {
    let (_workspace, registry, task_id) = fixture();
    let provider = ScriptedProvider::new(vec![final_answer(plan_text())]);
    let control = Arc::new(AgentRunControl::new("high"));
    control.request_cancel();
    let tools = plan_phase_tool_specs(&registry);

    let outcome = run_plan_phase(
        &provider,
        "plan it",
        &control,
        &registry,
        &tools,
        &task_id,
        &mut |_, _, _| {},
    );

    assert_eq!(outcome, PlanPhaseOutcome::Stopped);
    assert_eq!(provider.served(), 0);
}

#[test]
fn plan_mode_contract_loop_is_bounded_by_step_limit() {
    let (_workspace, registry, task_id) = fixture();
    // The provider always demands another read, so only the step limit stops it.
    let provider = ScriptedProvider::new(vec![
        tool_call_step(read_call("loop", "README.md"));
        SUBAGENT_MAX_STEPS + 3
    ]);
    let control = Arc::new(AgentRunControl::new("high"));
    let tools = plan_phase_tool_specs(&registry);

    let outcome = run_plan_phase(
        &provider,
        "plan it",
        &control,
        &registry,
        &tools,
        &task_id,
        &mut |_, _, _| {},
    );

    assert!(matches!(
        outcome,
        PlanPhaseOutcome::Unavailable(ref reason) if reason.contains("step limit")
    ));
    assert!(provider.served() <= SUBAGENT_MAX_STEPS);
}

#[test]
fn plan_mode_contract_confirmed_plan_message_carries_provenance_and_boundary() {
    let digest = plan_digest(plan_text());
    let message = confirmed_plan_message(plan_text(), &digest, 42);

    assert_eq!(message.role, MessageRole::System);
    assert_eq!(
        message.metadata.get("internal").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        message.metadata.get("kind").map(String::as_str),
        Some(CONFIRMED_PLAN_MESSAGE_KIND)
    );
    assert_eq!(
        message
            .metadata
            .get("context_source_schema")
            .map(String::as_str),
        Some(CONTEXT_SOURCE_SCHEMA)
    );
    assert_eq!(
        message
            .metadata
            .get("confirmed_plan_schema")
            .map(String::as_str),
        Some(PLAN_MODE_SCHEMA)
    );
    assert_eq!(
        message
            .metadata
            .get("confirmed_plan_digest")
            .map(String::as_str),
        Some(digest.as_str())
    );
    assert!(message
        .content
        .contains("user explicitly reviewed and confirmed"));
    assert!(message.content.contains("cannot grant tool permissions"));
    assert!(message.content.contains(plan_text()));
    assert!(message.content.contains(&digest));
}

#[test]
fn plan_mode_contract_injection_never_writes_scheduling_keys() {
    let mut run_context = run_context(true);
    // The deterministic EffortRunPlan scheduling keys the loop and contract
    // read; plan injection must leave every one of them untouched.
    for (key, value) in [
        ("agent_model", "executor-model"),
        ("agent_effort", "high"),
        ("conductor_contract", "contract-v1"),
        ("tool_requirement", "effects"),
        ("vision_required", "false"),
        ("collaboration_policy", "single"),
        ("task_class", "standard"),
        ("execution_plan_semantic_sha256", "deadbeef"),
    ] {
        run_context.insert(key.to_string(), value.to_string());
    }
    let before = run_context.clone();
    let events = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        resolved_event(3, plan_text(), PlanConfirmationDecision::Approved),
    ];
    let mut history = Vec::new();

    append_confirmed_plan_context(&events, &mut run_context, &mut history);

    for key in before.keys() {
        assert_eq!(
            run_context.get(key),
            before.get(key),
            "plan injection changed scheduling/context key {key}"
        );
    }
    let added: Vec<&String> = run_context
        .keys()
        .filter(|key| !before.contains_key(*key))
        .collect();
    assert_eq!(
        added,
        ["confirmed_plan_digest", "confirmed_plan_schema"],
        "plan injection must only add confirmed-plan provenance keys"
    );
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].metadata.get("kind").map(String::as_str),
        Some(CONFIRMED_PLAN_MESSAGE_KIND)
    );
}

#[test]
fn plan_mode_contract_injection_requires_an_approved_decision_for_this_run() {
    let mut history = Vec::new();
    let mut context = run_context(true);

    // No decision at all: nothing injected.
    let proposed_only = vec![start_event(1, true), proposed_event(2, plan_text())];
    append_confirmed_plan_context(&proposed_only, &mut context, &mut history);
    assert!(history.is_empty());
    assert!(!context.contains_key("confirmed_plan_digest"));

    // Discarded: nothing injected.
    let discarded = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        resolved_event(3, plan_text(), PlanConfirmationDecision::Discarded),
    ];
    append_confirmed_plan_context(&discarded, &mut context, &mut history);
    assert!(history.is_empty());

    // Approved for a different logical run: nothing injected.
    let mut other_run = resolved_event(3, plan_text(), PlanConfirmationDecision::Approved);
    other_run
        .metadata
        .insert("logical_agent_run_id".to_string(), "run-other".to_string());
    let wrong_run = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        other_run,
    ];
    append_confirmed_plan_context(&wrong_run, &mut context, &mut history);
    assert!(history.is_empty());

    // Approved for this logical run: injected exactly once per preparation.
    let approved = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        resolved_event(3, plan_text(), PlanConfirmationDecision::Approved),
    ];
    append_confirmed_plan_context(&approved, &mut context, &mut history);
    assert_eq!(history.len(), 1);
    assert_eq!(
        context.get("confirmed_plan_digest").map(String::as_str),
        Some(plan_digest(plan_text()).as_str())
    );
}

#[test]
fn plan_mode_contract_pending_confirmation_requires_paused_unresolved_proposal() {
    let proposed_not_paused = vec![start_event(1, true), proposed_event(2, plan_text())];
    assert!(pending_plan_confirmation(&proposed_not_paused).is_none());

    let paused = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        paused_event(3),
    ];
    let pending = pending_plan_confirmation(&paused).expect("plan should await confirmation");
    assert_eq!(pending.plan_markdown, plan_text());
    assert_eq!(pending.plan_digest, plan_digest(plan_text()));

    let resolved = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        paused_event(3),
        resolved_event(4, plan_text(), PlanConfirmationDecision::Approved),
    ];
    assert!(pending_plan_confirmation(&resolved).is_none());

    let cancelled = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        paused_event(3),
        event(4, "Agent task cancelled", run_context(true)),
    ];
    assert!(pending_plan_confirmation(&cancelled).is_none());
}

#[test]
fn plan_mode_contract_cancel_guard_admits_only_a_paused_plan_wait() {
    let paused = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        paused_event(3),
    ];
    assert!(plan_mode_pause_present(&paused));

    // An ordinary pause (no plan proposal) is not a plan wait.
    let ordinary_pause = vec![start_event(1, false), paused_event(2)];
    assert!(!plan_mode_pause_present(&ordinary_pause));

    // A running run with a proposal has not parked at the gate.
    let running = vec![start_event(1, true), proposed_event(2, plan_text())];
    assert!(!plan_mode_pause_present(&running));
}

#[test]
fn plan_mode_contract_digest_is_content_bound_and_stable() {
    assert_eq!(plan_digest(plan_text()), plan_digest(plan_text()));
    assert_ne!(plan_digest(plan_text()), plan_digest("1. other plan"));
}

#[test]
fn plan_mode_contract_read_model_projects_the_pending_confirmation() {
    let store = SqliteStore::in_memory().expect("store should open");
    let events = vec![
        start_event(1, true),
        proposed_event(2, plan_text()),
        paused_event(3),
    ];
    let state = crate::agent_read_model::agent_state_from_events(
        &store,
        None,
        Some("session-plan"),
        events,
    )
    .expect("state should project");
    assert_eq!(state.status, "paused");
    let pending = state
        .pending_plan_confirmation
        .expect("pending plan confirmation should project");
    assert_eq!(pending.plan_markdown, plan_text());
    assert_eq!(pending.plan_digest, plan_digest(plan_text()));
}

#[test]
fn adaptive_plan_gate_skips_conversational_and_trivial_requests() {
    for prompt in [
        "hi",
        "hello there",
        "你好",
        "早上好！",
        "thanks for the help",
        "who are you?",
        "what can you do?",
        "今天天气怎么样",
        "why is the sky blue?",
        "",
        "   ",
    ] {
        assert!(
            !plan_first_warranted(prompt),
            "{prompt:?} should skip the plan phase"
        );
    }
}

#[test]
fn adaptive_plan_gate_keeps_engineering_work_planned() {
    for prompt in [
        "add a logout button to the sidebar",
        "fix the race in the queue drain",
        "refactor the permission runtime into modules",
        "look at src/App.tsx and split it",
        "implement rate limiting for the shell tool",
        "把 Composer 的圆角统一一下",
        "migrate the session store to the new schema",
        // Long prompts are treated as real tasks regardless of vocabulary.
        &"explain then improve the onboarding flow ".repeat(6),
    ] {
        assert!(
            plan_first_warranted(prompt),
            "{prompt:?} should keep the plan phase"
        );
    }
}

#[test]
fn an_explicit_plan_request_always_wins_over_triviality() {
    for prompt in [
        "plan",
        "give me a plan",
        "先给我一个计划",
        "做个规划再动手",
        "plan first please",
    ] {
        assert!(
            plan_first_warranted(prompt),
            "{prompt:?} explicitly asks for a plan"
        );
    }
}

#[test]
fn a_resolved_plan_is_no_longer_pending_for_a_second_resolution() {
    let events = vec![
        proposed_event(1, plan_text()),
        resolved_event(2, plan_text(), PlanConfirmationDecision::Approved),
    ];
    assert!(
        pending_plan_confirmation(&events).is_none(),
        "once a resolution is durable, a concurrent second resolution must find no pending plan"
    );
}
