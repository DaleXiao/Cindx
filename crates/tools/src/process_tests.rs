use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use agent_core::{
    permission_can_allow_session, Metadata, TaskId, ToolCallId, ToolEffectSemantics,
    ToolInvocation, ToolOutcomeStatus, ToolRisk,
};

use crate::{
    ProcessInputTool, ProcessManager, ProcessPollTool, ProcessStartTool, ProcessTerminateTool,
    Tool, ToolExecutionControl, ToolRegistry,
};

fn workspace() -> tempfile::TempDir {
    tempfile::tempdir().expect("workspace should be created")
}

fn invocation(
    tool_name: &str,
    call_id: &str,
    input: serde_json::Value,
    run_id: &str,
) -> ToolInvocation {
    ToolInvocation {
        id: ToolCallId(call_id.to_string()),
        task_id: TaskId("process-task".to_string()),
        tool_name: tool_name.to_string(),
        input_json: input.to_string(),
        proposed_by_model: "test".to_string(),
        metadata: [
            ("project_id".to_string(), "project-a".to_string()),
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
            ("prompt_contract_epoch".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>(),
    }
}

fn process_id(result: &agent_core::ToolResult) -> String {
    serde_json::from_str::<serde_json::Value>(
        result
            .structured_output_json
            .as_deref()
            .expect("process result should be structured"),
    )
    .expect("process result should be JSON")
    .get("process_id")
    .and_then(serde_json::Value::as_str)
    .expect("process id should exist")
    .to_string()
}

fn poll_until_terminal(
    tool: &ProcessPollTool,
    process_id: &str,
    run_id: &str,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut attempt = 0;
    loop {
        let result = tool
            .execute(invocation(
                "process.poll",
                &format!("poll-{attempt}"),
                serde_json::json!({
                    "process_id": process_id,
                    "max_bytes": 65_536,
                    "wait_ms": 200
                }),
                run_id,
            ))
            .expect("poll should execute");
        assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
        let structured = serde_json::from_str::<serde_json::Value>(
            result
                .structured_output_json
                .as_deref()
                .expect("poll should be structured"),
        )
        .expect("poll result should be JSON");
        if structured.get("state").and_then(serde_json::Value::as_str) == Some("terminal") {
            return structured;
        }
        assert!(Instant::now() < deadline, "process should become terminal");
        attempt += 1;
    }
}

fn tools(
    root: &Path,
) -> (
    Arc<ProcessManager>,
    ProcessStartTool,
    ProcessPollTool,
    ProcessInputTool,
    ProcessTerminateTool,
) {
    let manager = Arc::new(ProcessManager::new());
    (
        Arc::clone(&manager),
        ProcessStartTool::new(root, Arc::clone(&manager)),
        ProcessPollTool::new(Arc::clone(&manager)),
        ProcessInputTool::new(Arc::clone(&manager)),
        ProcessTerminateTool::new(Arc::clone(&manager)),
    )
}

#[test]
fn process_plane_registers_four_typed_non_verifier_tools() {
    let root = workspace();
    let registry = ToolRegistry::with_workspace_tools(root.path());
    for name in [
        "process.start",
        "process.poll",
        "process.input",
        "process.terminate",
    ] {
        let spec = registry
            .specs()
            .into_iter()
            .find(|spec| spec.name == name)
            .unwrap_or_else(|| panic!("{name} should be registered"));
        assert_eq!(spec.risk, ToolRisk::ExecutesProcess);
        assert!(spec.output_schema_json.is_some());
        assert!(spec.postcondition_verifiers.is_empty());
        if name == "process.poll" {
            assert_eq!(spec.effect_semantics, ToolEffectSemantics::Idempotent);
        }
    }
}

#[test]
fn process_start_is_durable_pending_before_any_command_effect() {
    let root = workspace();
    let marker = root.path().join("activated.txt");
    let (_manager, start, poll, _input, _terminate) = tools(root.path());
    let result = start
        .execute(invocation(
            "process.start",
            "start-pending",
            serde_json::json!({
                "command": "printf activated > activated.txt",
                "timeout_seconds": 5
            }),
            "run-a",
        ))
        .expect("start should reserve a process");
    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert!(!marker.exists(), "reservation must not execute the command");

    let terminal = poll_until_terminal(&poll, &process_id(&result), "run-a");
    assert_eq!(
        terminal
            .get("termination")
            .and_then(serde_json::Value::as_str),
        Some("exit")
    );
    assert_eq!(
        fs::read_to_string(marker).expect("activated marker should be readable"),
        "activated"
    );
}

#[test]
fn long_process_poll_is_incremental_and_non_blocking() {
    let root = workspace();
    let (_manager, start, poll, _input, terminate) = tools(root.path());
    let started = start
        .execute(invocation(
            "process.start",
            "start-long",
            serde_json::json!({"command": "sleep 5", "timeout_seconds": 10}),
            "run-a",
        ))
        .expect("start should succeed");
    let id = process_id(&started);
    let began = Instant::now();
    let result = poll
        .execute(invocation(
            "process.poll",
            "poll-long",
            serde_json::json!({"process_id": id}),
            "run-a",
        ))
        .expect("poll should succeed");
    assert!(began.elapsed() < Duration::from_millis(500));
    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    let stopped = terminate
        .execute(invocation(
            "process.terminate",
            "terminate-long",
            serde_json::json!({"process_id": id}),
            "run-a",
        ))
        .expect("terminate should succeed");
    assert_eq!(stopped.status, ToolOutcomeStatus::Succeeded);
}

#[test]
fn process_input_is_acknowledged_and_observed_by_poll() {
    let root = workspace();
    let (_manager, start, poll, input, _terminate) = tools(root.path());
    let started = start
        .execute(invocation(
            "process.start",
            "start-input",
            serde_json::json!({"command": "read line; printf 'got:%s' \"$line\""}),
            "run-a",
        ))
        .expect("start should succeed");
    let id = process_id(&started);
    let written = input
        .execute(invocation(
            "process.input",
            "input-line",
            serde_json::json!({"process_id": id, "text": "hello\n", "close": true}),
            "run-a",
        ))
        .expect("input should execute");
    assert_eq!(written.status, ToolOutcomeStatus::Succeeded);
    let terminal = poll_until_terminal(&poll, &id, "run-a");
    assert_eq!(
        terminal
            .pointer("/stdout/text")
            .and_then(serde_json::Value::as_str),
        Some("got:hello")
    );
}

#[test]
fn process_input_permission_is_payload_bound_and_never_session_reusable() {
    let root = workspace();
    let (_manager, _start, _poll, input, _terminate) = tools(root.path());
    let request = input
        .permission_request(&invocation(
            "process.input",
            "input-permission",
            serde_json::json!({
                "process_id": "proc_000000000000000000000000000000000000",
                "text": "continue\n"
            }),
            "run-a",
        ))
        .expect("process input must request one-shot permission");
    assert_eq!(request.action, "process.input");
    assert_eq!(
        request.metadata.get("session_reusable").map(String::as_str),
        Some("false")
    );
    assert!(request.metadata.contains_key("input_digest"));
    assert!(!permission_can_allow_session(&request));
}

#[test]
fn terminating_a_pending_reservation_never_activates_it_and_closes_receipt_streams() {
    let root = workspace();
    let marker = root.path().join("must-not-activate.txt");
    let (_manager, start, _poll, _input, terminate) = tools(root.path());
    let started = start
        .execute(invocation(
            "process.start",
            "start-pending-terminate",
            serde_json::json!({"command": "touch must-not-activate.txt"}),
            "run-a",
        ))
        .expect("start should reserve a process");
    let result = terminate
        .execute(invocation(
            "process.terminate",
            "terminate-pending",
            serde_json::json!({"process_id": process_id(&started)}),
            "run-a",
        ))
        .expect("pending terminate should succeed");
    let structured = serde_json::from_str::<serde_json::Value>(
        result
            .structured_output_json
            .as_deref()
            .expect("terminate should be structured"),
    )
    .expect("terminate result should be JSON");
    assert_eq!(
        structured
            .get("termination")
            .and_then(serde_json::Value::as_str),
        Some("terminated")
    );
    assert_eq!(
        structured
            .pointer("/stdout/complete")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert!(!marker.exists());
}

#[test]
fn process_handle_isolated_by_physical_run() {
    let root = workspace();
    let (_manager, start, poll, _input, terminate) = tools(root.path());
    let started = start
        .execute(invocation(
            "process.start",
            "start-owner",
            serde_json::json!({"command": "sleep 5"}),
            "run-a",
        ))
        .expect("start should succeed");
    let id = process_id(&started);
    let denied = poll
        .execute(invocation(
            "process.poll",
            "poll-wrong-owner",
            serde_json::json!({"process_id": id}),
            "run-b",
        ))
        .expect("poll transport should complete");
    assert_eq!(denied.status, ToolOutcomeStatus::Failed);
    assert_eq!(
        denied.failure.as_ref().map(|failure| failure.code.as_str()),
        Some("process_not_found")
    );
    let _ = terminate.execute(invocation(
        "process.terminate",
        "terminate-owner",
        serde_json::json!({"process_id": id}),
        "run-a",
    ));
}

#[test]
fn process_output_budget_stops_runaway_output() {
    let root = workspace();
    let (_manager, start, poll, _input, _terminate) = tools(root.path());
    let started = start
        .execute(invocation(
            "process.start",
            "start-output-limit",
            serde_json::json!({
                "command": "while true; do printf '0123456789abcdef'; done",
                "output_limit_bytes": 4096,
                "timeout_seconds": 5
            }),
            "run-a",
        ))
        .expect("start should succeed");
    let terminal = poll_until_terminal(&poll, &process_id(&started), "run-a");
    assert_eq!(
        terminal
            .get("termination")
            .and_then(serde_json::Value::as_str),
        Some("output_limit")
    );
    let captured = terminal
        .pointer("/stdout/captured_bytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default()
        + terminal
            .pointer("/stderr/captured_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
    assert!(captured <= 4096);
    assert_eq!(
        terminal
            .pointer("/stdout/truncated")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[test]
fn nonzero_exit_is_a_typed_child_outcome_not_a_transport_failure() {
    let root = workspace();
    let (_manager, start, poll, _input, _terminate) = tools(root.path());
    let started = start
        .execute(invocation(
            "process.start",
            "start-nonzero",
            serde_json::json!({"command": "printf failed >&2; exit 7"}),
            "run-a",
        ))
        .expect("start should succeed");
    let terminal = poll_until_terminal(&poll, &process_id(&started), "run-a");
    assert_eq!(
        terminal
            .get("termination")
            .and_then(serde_json::Value::as_str),
        Some("exit")
    );
    assert_eq!(
        terminal
            .get("exit_code")
            .and_then(serde_json::Value::as_i64),
        Some(7)
    );
    assert_eq!(
        terminal
            .pointer("/stderr/text")
            .and_then(serde_json::Value::as_str),
        Some("failed")
    );
}

#[cfg(unix)]
#[test]
fn process_cpu_budget_is_enforced_by_the_child_runtime() {
    let root = workspace();
    let (_manager, start, poll, _input, _terminate) = tools(root.path());
    let started = start
        .execute(invocation(
            "process.start",
            "start-cpu-limit",
            serde_json::json!({
                "command": "yes > /dev/null",
                "cpu_seconds": 1,
                "timeout_seconds": 5
            }),
            "run-a",
        ))
        .expect("start should succeed");
    let terminal = poll_until_terminal(&poll, &process_id(&started), "run-a");
    assert_eq!(
        terminal
            .get("termination")
            .and_then(serde_json::Value::as_str),
        Some("cpu_limit")
    );
}

#[test]
fn epoch_cancellation_stops_running_process_and_descendants() {
    let root = workspace();
    let marker = root.path().join("should-not-exist.txt");
    let (_manager, start, poll, _input, _terminate) = tools(root.path());
    let cancelled = Arc::new(AtomicBool::new(false));
    let control = ToolExecutionControl::new({
        let cancelled = Arc::clone(&cancelled);
        move || cancelled.load(Ordering::SeqCst)
    });
    let started = start
        .execute_with_control(
            invocation(
                "process.start",
                "start-cancel",
                serde_json::json!({
                    "command": "(sleep 1; touch should-not-exist.txt) & sleep 5",
                    "timeout_seconds": 10
                }),
                "run-a",
            ),
            &control,
        )
        .expect("start should succeed");
    let id = process_id(&started);
    let _ = poll.execute(invocation(
        "process.poll",
        "poll-activate-cancel",
        serde_json::json!({"process_id": id}),
        "run-a",
    ));
    cancelled.store(true, Ordering::SeqCst);
    let terminal = poll_until_terminal(&poll, &id, "run-a");
    assert_eq!(
        terminal
            .get("termination")
            .and_then(serde_json::Value::as_str),
        Some("cancelled")
    );
    std::thread::sleep(Duration::from_millis(1_200));
    assert!(!marker.exists(), "cancel must stop the whole process group");
}

#[test]
fn physical_run_completion_reaps_owned_processes_and_descendants() {
    let root = workspace();
    let marker = root.path().join("run-ended-child.txt");
    let (manager, start, poll, _input, _terminate) = tools(root.path());
    let started = start
        .execute(invocation(
            "process.start",
            "start-run-ended",
            serde_json::json!({
                "command": "(sleep 1; touch run-ended-child.txt) & sleep 5",
                "timeout_seconds": 10
            }),
            "run-a",
        ))
        .expect("start should succeed");
    let id = process_id(&started);
    let _ = poll.execute(invocation(
        "process.poll",
        "poll-run-ended",
        serde_json::json!({"process_id": id}),
        "run-a",
    ));
    let run_context = invocation("process.poll", "owner", serde_json::json!({}), "run-a").metadata;
    manager.shutdown_run(&TaskId("process-task".to_string()), &run_context);
    let terminal = poll_until_terminal(&poll, &id, "run-a");
    assert_eq!(
        terminal
            .get("termination")
            .and_then(serde_json::Value::as_str),
        Some("run_ended")
    );
    std::thread::sleep(Duration::from_millis(1_200));
    assert!(
        !marker.exists(),
        "run completion must reap the process group"
    );
}

#[test]
fn terminate_waits_for_activation_and_cannot_lose_the_stop_request() {
    let root = workspace();
    let marker = root.path().join("activation-race-child.txt");
    let (manager, start, poll, _input, terminate) = tools(root.path());
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    manager.set_activation_hook(Arc::new({
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        move || {
            entered.wait();
            release.wait();
        }
    }));
    let started = start
        .execute(invocation(
            "process.start",
            "start-activation-race",
            serde_json::json!({
                "command": "(sleep 1; touch activation-race-child.txt) & sleep 5",
                "timeout_seconds": 10
            }),
            "run-a",
        ))
        .expect("start should reserve a process");
    let id = process_id(&started);
    let poll = Arc::new(poll);
    let poll_thread = std::thread::spawn({
        let poll = Arc::clone(&poll);
        let id = id.clone();
        move || {
            poll.execute(invocation(
                "process.poll",
                "poll-activation-race",
                serde_json::json!({"process_id": id}),
                "run-a",
            ))
        }
    });
    entered.wait();
    let terminate = Arc::new(terminate);
    let terminate_thread = std::thread::spawn({
        let terminate = Arc::clone(&terminate);
        let id = id.clone();
        move || {
            terminate.execute(invocation(
                "process.terminate",
                "terminate-activation-race",
                serde_json::json!({"process_id": id}),
                "run-a",
            ))
        }
    });
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        !terminate_thread.is_finished(),
        "termination must serialize with activation"
    );
    release.wait();
    poll_thread
        .join()
        .expect("poll thread should join")
        .expect("poll should execute");
    let stopped = terminate_thread
        .join()
        .expect("terminate thread should join")
        .expect("terminate should execute");
    assert_eq!(stopped.status, ToolOutcomeStatus::Succeeded);
    let terminal = poll_until_terminal(poll.as_ref(), &id, "run-a");
    assert_eq!(
        terminal
            .get("termination")
            .and_then(serde_json::Value::as_str),
        Some("terminated")
    );
    std::thread::sleep(Duration::from_millis(1_200));
    assert!(
        !marker.exists(),
        "termination must reap activated descendants"
    );
}

#[test]
fn process_owner_concurrency_limit_is_applied_before_activation() {
    let root = workspace();
    let (manager, start, _poll, _input, _terminate) = tools(root.path());
    for index in 0..2 {
        let result = start
            .execute(invocation(
                "process.start",
                &format!("start-{index}"),
                serde_json::json!({"command": "sleep 5"}),
                "run-a",
            ))
            .expect("reservation should complete");
        assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    }
    let rejected = start
        .execute(invocation(
            "process.start",
            "start-over-limit",
            serde_json::json!({"command": "sleep 5"}),
            "run-a",
        ))
        .expect("limit should be a typed result");
    assert_eq!(rejected.status, ToolOutcomeStatus::Failed);
    assert_eq!(
        rejected
            .failure
            .as_ref()
            .map(|failure| failure.code.as_str()),
        Some("process_owner_concurrency_limit")
    );
    manager.shutdown_all();
}

#[test]
fn process_children_do_not_inherit_unlisted_credentials() {
    let root = workspace();
    let (_manager, start, poll, _input, _terminate) = tools(root.path());
    const SECRET: &str = "CINDX_PROCESS_TEST_API_TOKEN";
    std::env::set_var(SECRET, "must-not-leak");
    let started = start
        .execute(invocation(
            "process.start",
            "start-environment",
            serde_json::json!({"command": format!("printf %s \"${SECRET}\"")}),
            "run-a",
        ))
        .expect("start should succeed");
    let terminal = poll_until_terminal(&poll, &process_id(&started), "run-a");
    std::env::remove_var(SECRET);
    assert_eq!(
        terminal
            .pointer("/stdout/text")
            .and_then(serde_json::Value::as_str),
        Some("")
    );
}

#[test]
fn process_session_contract_gate() {
    process_plane_registers_four_typed_non_verifier_tools();
    process_start_is_durable_pending_before_any_command_effect();
    long_process_poll_is_incremental_and_non_blocking();
    process_input_is_acknowledged_and_observed_by_poll();
    process_input_permission_is_payload_bound_and_never_session_reusable();
    terminating_a_pending_reservation_never_activates_it_and_closes_receipt_streams();
    process_handle_isolated_by_physical_run();
    process_output_budget_stops_runaway_output();
    nonzero_exit_is_a_typed_child_outcome_not_a_transport_failure();
    #[cfg(unix)]
    process_cpu_budget_is_enforced_by_the_child_runtime();
    epoch_cancellation_stops_running_process_and_descendants();
    physical_run_completion_reaps_owned_processes_and_descendants();
    terminate_waits_for_activation_and_cannot_lose_the_stop_request();
    process_owner_concurrency_limit_is_applied_before_activation();
    process_children_do_not_inherit_unlisted_credentials();
    eprintln!("cindx.process-session-contract.v1");
}

#[cfg(unix)]
#[test]
fn process_spawn_argv_honors_the_session_sandbox_mode() {
    use crate::process_supervisor::process_spawn_argv;
    use agent_core::SandboxMode;

    let root = Path::new("/workspace");

    // FullAccess preserves the exact historical argv (no wrapper process).
    let open = process_spawn_argv("echo hi", SandboxMode::FullAccess, root);
    assert_eq!(open[0], "/bin/zsh");
    assert_eq!(open[1], "-fc");
    assert!(!open.iter().any(|token| token == "sandbox-exec"));

    // ReadOnly wraps the wrapper shell in sandbox-exec with the deny-write
    // profile, exactly like shell.run.
    let read_only = process_spawn_argv("echo hi", SandboxMode::ReadOnly, root);
    assert_eq!(read_only[0], "/usr/bin/sandbox-exec");
    assert_eq!(read_only[1], "-p");
    assert!(read_only[2].contains("(deny file-write*)"));
    assert_eq!(read_only[3], "--");
    assert_eq!(read_only[4], "/bin/zsh");
    assert_eq!(read_only[5], "-fc");

    // WorkspaceWrite additionally allows writes inside the workspace root.
    let workspace_write = process_spawn_argv("echo hi", SandboxMode::WorkspaceWrite, root);
    assert!(workspace_write[2].contains("(subpath \"/workspace\")"));
}
