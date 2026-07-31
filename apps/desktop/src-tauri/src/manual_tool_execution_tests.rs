use crate::{append_event, execute_manual_tool_invocation};
use agent_core::{
    EventKind, Metadata, PermissionRequest, TaskId, ToolCallId, ToolInvocation, ToolOutcomeStatus,
    ToolResult, ToolRisk, ToolSpec,
};
use agent_storage::SqliteStore;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;
use tools::{Tool, ToolError, ToolRegistry};

struct BlockingTool {
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    count: Arc<AtomicUsize>,
}

impl Tool for BlockingTool {
    fn spec(&self) -> ToolSpec {
        test_tool_spec("test.blocking")
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.entered
            .send(())
            .map_err(|_| ToolError::new("test observer disconnected"))?;
        self.release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| ToolError::new("test release timed out"))?;
        Ok(ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            "completed outside the store lock",
            Metadata::new(),
        ))
    }
}

struct CountingTool {
    count: Arc<AtomicUsize>,
    fails: bool,
}

impl Tool for CountingTool {
    fn spec(&self) -> ToolSpec {
        test_tool_spec(if self.fails {
            "test.failure"
        } else {
            "test.counting"
        })
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        if self.fails {
            return Err(ToolError::retryable(
                "temporary_test_failure",
                "retryable tool failure",
            ));
        }
        Ok(ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            "executed once",
            Metadata::new(),
        ))
    }
}

fn test_tool_spec(name: &str) -> ToolSpec {
    ToolSpec::builtin(
        name,
        "test",
        "manual tool execution test",
        ToolRisk::ReadOnly,
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    )
}

fn invocation(id: &str, tool_name: &str) -> ToolInvocation {
    ToolInvocation {
        id: ToolCallId(id.to_string()),
        task_id: TaskId("manual-tool-test".to_string()),
        tool_name: tool_name.to_string(),
        input_json: "{}".to_string(),
        proposed_by_model: "test".to_string(),
        metadata: [("session_id".to_string(), "session-test".to_string())]
            .into_iter()
            .collect(),
    }
}

#[test]
fn manual_tool_body_releases_store_lock_and_commits_success() {
    let directory = tempfile::tempdir().expect("temporary workspace should open");
    let workspace_root = directory.path().to_path_buf();
    let store = Arc::new(Mutex::new(
        SqliteStore::in_memory().expect("store should open"),
    ));
    let execution_gate = Arc::new(Mutex::new(()));
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let count = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(BlockingTool {
        entered: entered_tx,
        release: Mutex::new(release_rx),
        count: Arc::clone(&count),
    }));
    let invocation = invocation("manual-blocking", "test.blocking");

    let worker_store = Arc::clone(&store);
    let worker_execution_gate = Arc::clone(&execution_gate);
    let worker_registry = registry.clone();
    let worker_invocation = invocation.clone();
    let worker = std::thread::spawn(move || {
        execute_manual_tool_invocation(
            &worker_execution_gate,
            &worker_store,
            &worker_registry,
            worker_invocation,
            &workspace_root,
            None,
        )
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("tool body should start");
    assert!(
        execution_gate.try_lock().is_err(),
        "the dedicated manual gate must preserve serialized tool execution"
    );

    {
        let mut available = store
            .try_lock()
            .expect("store lock must be free while the tool body is blocked");
        let events = available
            .list_by_task_and_tool_call_id(&invocation.task_id, &invocation.id.0)
            .expect("started event should load");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::ToolCallStarted);
        append_event(
            &mut available,
            &TaskId("concurrent-store-user".to_string()),
            EventKind::TaskStatusChanged,
            "Concurrent store work committed",
            Metadata::new(),
        )
        .expect("another store writer should make progress");
    }

    release_tx.send(()).expect("tool body should be released");
    let result = worker
        .join()
        .expect("manual tool worker should not panic")
        .expect("manual tool should complete");
    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let guard = store.lock().expect("store should remain available");
    let events = guard
        .list_by_task_and_tool_call_id(&invocation.task_id, &invocation.id.0)
        .expect("tool events should load");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, EventKind::ToolCallStarted);
    assert_eq!(events[1].kind, EventKind::ToolCallFinished);
    assert_eq!(
        events[1].metadata.get("status").map(String::as_str),
        Some("succeeded")
    );
    assert_eq!(
        events[1].metadata.get("session_id").map(String::as_str),
        Some("session-test")
    );
}

#[test]
fn manual_tool_failure_is_committed_with_retryability() {
    let directory = tempfile::tempdir().expect("temporary workspace should open");
    let store = Mutex::new(SqliteStore::in_memory().expect("store should open"));
    let execution_gate = Mutex::new(());
    let count = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingTool {
        count: Arc::clone(&count),
        fails: true,
    }));
    let invocation = invocation("manual-failure", "test.failure");

    let result = execute_manual_tool_invocation(
        &execution_gate,
        &store,
        &registry,
        invocation.clone(),
        directory.path(),
        None,
    )
    .expect("tool failures should become committed results");

    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(result.status, ToolOutcomeStatus::Failed);
    assert_eq!(
        result.failure.as_ref().map(|failure| failure.code.as_str()),
        Some("temporary_test_failure")
    );
    assert!(result
        .failure
        .as_ref()
        .is_some_and(|failure| failure.retryable));
    let guard = store.lock().expect("store should remain available");
    let finished = guard
        .list_by_task_and_tool_call_id(&invocation.task_id, &invocation.id.0)
        .expect("tool events should load")
        .into_iter()
        .find(|event| event.kind == EventKind::ToolCallFinished)
        .expect("failure should be committed");
    assert_eq!(
        finished.metadata.get("status").map(String::as_str),
        Some("failed")
    );
    assert_eq!(
        finished
            .metadata
            .get("result_failure_code")
            .map(String::as_str),
        Some("temporary_test_failure")
    );
    assert_eq!(
        finished
            .metadata
            .get("result_failure_retryable")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn manual_tool_exact_replay_does_not_execute_twice() {
    let directory = tempfile::tempdir().expect("temporary workspace should open");
    let store = Mutex::new(SqliteStore::in_memory().expect("store should open"));
    let execution_gate = Mutex::new(());
    let count = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingTool {
        count: Arc::clone(&count),
        fails: false,
    }));
    let invocation = invocation("manual-replay", "test.counting");

    let first = execute_manual_tool_invocation(
        &execution_gate,
        &store,
        &registry,
        invocation.clone(),
        directory.path(),
        None,
    )
    .expect("first execution should complete");
    let replayed = execute_manual_tool_invocation(
        &execution_gate,
        &store,
        &registry,
        invocation.clone(),
        directory.path(),
        None,
    )
    .expect("exact replay should load the completed result");

    assert_eq!(first.output, replayed.output);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        replayed
            .metadata
            .get("idempotent_replay")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        replayed
            .metadata
            .get("effect_replay_mode")
            .map(String::as_str),
        Some("exact_call_id")
    );
    let guard = store.lock().expect("store should remain available");
    let events = guard
        .list_by_task_and_tool_call_id(&invocation.task_id, &invocation.id.0)
        .expect("tool events should load");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::ToolCallStarted)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::ToolCallFinished)
            .count(),
        1
    );
}

#[test]
fn concurrent_manual_tool_replay_waits_for_commit_and_executes_once() {
    let directory = tempfile::tempdir().expect("temporary workspace should open");
    let workspace_root = directory.path().to_path_buf();
    let store = Arc::new(Mutex::new(
        SqliteStore::in_memory().expect("store should open"),
    ));
    let execution_gate = Arc::new(Mutex::new(()));
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let count = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(BlockingTool {
        entered: entered_tx,
        release: Mutex::new(release_rx),
        count: Arc::clone(&count),
    }));
    let invocation = invocation("manual-concurrent-replay", "test.blocking");

    let first = {
        let store = Arc::clone(&store);
        let execution_gate = Arc::clone(&execution_gate);
        let registry = registry.clone();
        let invocation = invocation.clone();
        let workspace_root = workspace_root.clone();
        std::thread::spawn(move || {
            execute_manual_tool_invocation(
                &execution_gate,
                &store,
                &registry,
                invocation,
                &workspace_root,
                None,
            )
        })
    };
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("first tool body should start");

    let (second_ready_tx, second_ready_rx) = mpsc::sync_channel(1);
    let second = {
        let store = Arc::clone(&store);
        let execution_gate = Arc::clone(&execution_gate);
        let registry = registry.clone();
        let invocation = invocation.clone();
        let workspace_root = workspace_root.clone();
        std::thread::spawn(move || {
            second_ready_tx
                .send(())
                .expect("second worker observer should remain connected");
            execute_manual_tool_invocation(
                &execution_gate,
                &store,
                &registry,
                invocation,
                &workspace_root,
                None,
            )
        })
    };
    second_ready_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("second worker should be ready");
    assert!(
        execution_gate.try_lock().is_err(),
        "the second manual call must wait for the first commit"
    );
    assert!(
        store.try_lock().is_ok(),
        "the global store must remain available while the first body is blocked"
    );

    release_tx.send(()).expect("first tool body should release");
    let first_result = first
        .join()
        .expect("first manual tool worker should not panic")
        .expect("first execution should complete");
    let replayed = second
        .join()
        .expect("second manual tool worker should not panic")
        .expect("second execution should replay the committed result");

    assert_eq!(first_result.output, replayed.output);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        replayed
            .metadata
            .get("idempotent_replay")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        replayed
            .metadata
            .get("effect_replay_mode")
            .map(String::as_str),
        Some("exact_call_id")
    );
    let guard = store.lock().expect("store should remain available");
    let events = guard
        .list_by_task_and_tool_call_id(&invocation.task_id, &invocation.id.0)
        .expect("tool events should load");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::ToolCallStarted)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::ToolCallFinished)
            .count(),
        1
    );
}
