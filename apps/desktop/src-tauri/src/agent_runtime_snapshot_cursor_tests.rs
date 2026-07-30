use super::*;
use crate::agent_runtime_snapshot::capture_persistable_agent_task_state;
use agent_core::{Message, MessageRole, Metadata, TaskId};
use agent_runtime::{
    append_internal_instruction, start_agent_loop, AgentKernel, AgentLoopAppendTransaction,
    AgentRuntimeConfig,
};

fn run_context() -> Metadata {
    [
        ("session_id".to_string(), "session-a".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect()
}

fn runtime() -> agent_runtime::AgentLoopState {
    start_agent_loop(
        TaskId("task-a".to_string()),
        "api_key=secret-value",
        AgentRuntimeConfig::default(),
    )
}

fn assert_matches_full_capture(
    runtime: &agent_runtime::AgentLoopState,
    cursor: &AgentRuntimeSnapshotCursor,
    context: &Metadata,
) {
    let (incremental, _) = cursor.capture_current(runtime, context);
    assert_eq!(incremental, capture_persistable_agent_task_state(runtime));
}

#[test]
fn incremental_snapshot_matches_full_redacted_projection() {
    let context = run_context();
    let mut runtime = runtime();
    runtime.messages.extend([
        Message {
            role: MessageRole::Assistant,
            content: "<think>private</think>Visible sk-secret-value".to_string(),
            metadata: [
                (
                    "display_content".to_string(),
                    "<think>hidden</think>Visible".to_string(),
                ),
                ("tool_call_ids".to_string(), "call-a".to_string()),
                (
                    "raw_tool_calls_json".to_string(),
                    r#"[{"id":"call-a","arguments":{"api_key":"secret"}}]"#.to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::Tool,
            content: "authorization: bearer secret".to_string(),
            metadata: [
                ("kind".to_string(), "tool_observation".to_string()),
                ("tool_call_id".to_string(), "call-a".to_string()),
                ("status".to_string(), "succeeded".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::System,
            content: "transient context".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "context_restore_pack".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::Tool,
            content: "interrupted".to_string(),
            metadata: [("kind".to_string(), "recovery_observation".to_string())]
                .into_iter()
                .collect(),
        },
    ]);

    let cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);
    assert_matches_full_capture(&runtime, &cursor, &context);
}

#[test]
fn retry_truncate_and_steer_suffixes_match_full_capture() {
    let context = run_context();
    let mut runtime = runtime();
    runtime.messages.push(Message {
        role: MessageRole::Assistant,
        content: String::new(),
        metadata: [("tool_call_ids".to_string(), "call-a,call-b".to_string())]
            .into_iter()
            .collect(),
    });
    let mut cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);

    let retry_prefix = runtime.messages.len();
    runtime.messages.push(Message {
        role: MessageRole::Assistant,
        content: "discarded partial response".to_string(),
        metadata: Metadata::new(),
    });
    runtime.messages.truncate(retry_prefix);
    append_internal_instruction(
        &mut runtime,
        "model_response_retry",
        "Continue without repeating the partial response.",
    );
    let (retry_snapshot, retry_cursor) =
        cursor.prepare_after_append(&runtime, retry_prefix, &context);
    assert_eq!(
        retry_snapshot.task_state,
        capture_persistable_agent_task_state(&runtime)
    );
    cursor = retry_cursor;

    let steer_prefix = runtime.messages.len();
    AgentKernel::new(&mut runtime, &[]).apply_steer(
        "Use the revised objective",
        [("queue_id".to_string(), "queue-a".to_string())]
            .into_iter()
            .collect(),
    );
    let (steer_snapshot, cursor) = cursor.prepare_after_append(&runtime, steer_prefix, &context);
    assert_eq!(
        steer_snapshot.task_state,
        capture_persistable_agent_task_state(&runtime)
    );
    assert_matches_full_capture(&runtime, &cursor, &context);
}

#[test]
fn failed_runtime_transaction_does_not_publish_candidate_cursor() {
    let context = run_context();
    let mut runtime = runtime();
    let cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);
    let before = runtime.clone();
    let before_visits = cursor.message_visits();

    {
        let mut transaction = AgentLoopAppendTransaction::begin(&mut runtime);
        let prefix = transaction.original_message_count();
        append_internal_instruction(transaction.state_mut(), "model_response_retry", "candidate");
        let (_prepared, candidate) =
            cursor.prepare_after_append(transaction.state(), prefix, &context);
        assert_eq!(candidate.message_visits(), before_visits + 1);
        // Simulate persistence failure: neither guard nor cursor is committed.
    }

    assert_eq!(runtime, before);
    assert_eq!(cursor.message_visits(), before_visits);
    assert_matches_full_capture(&runtime, &cursor, &context);
}

#[test]
fn prefix_replacement_forces_safe_full_rebuild() {
    let context = run_context();
    let mut runtime = runtime();
    let cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);
    runtime.messages[0].content = "replacement request".to_string();

    let (prepared, rebuilt) = cursor.prepare_after_append(&runtime, 0, &context);
    assert_eq!(rebuilt.message_visits(), runtime.messages.len());
    assert_eq!(
        prepared.task_state,
        capture_persistable_agent_task_state(&runtime)
    );
}

#[test]
fn lineage_change_forces_safe_full_rebuild() {
    let context = run_context();
    let runtime = runtime();
    let cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);
    let mut changed_context = context.clone();
    changed_context.insert("agent_run_id".to_string(), "run-b".to_string());

    let (prepared, rebuilt) =
        cursor.prepare_after_append(&runtime, runtime.messages.len(), &changed_context);
    assert_eq!(rebuilt.message_visits(), runtime.messages.len());
    assert_eq!(
        prepared.task_state,
        capture_persistable_agent_task_state(&runtime)
    );
}

#[test]
fn incremental_runtime_snapshot_visits_only_appended_messages() {
    const APPENDS: usize = 1_152;
    let context = run_context();
    let mut runtime = runtime();
    let mut cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);
    let initial_visits = cursor.message_visits();

    for index in 0..APPENDS {
        let prefix = runtime.messages.len();
        runtime.messages.push(Message {
            role: MessageRole::Tool,
            content: format!("observation-{index}"),
            metadata: [
                ("kind".to_string(), "tool_observation".to_string()),
                ("tool_call_id".to_string(), format!("call-{index}")),
                ("status".to_string(), "succeeded".to_string()),
            ]
            .into_iter()
            .collect(),
        });
        let (_prepared, next) = cursor.prepare_after_append(&runtime, prefix, &context);
        cursor = next;
    }

    let historical_full_scan_visits = APPENDS
        .saturating_mul(initial_visits)
        .saturating_add(APPENDS.saturating_mul(APPENDS + 1) / 2);
    assert_eq!(cursor.message_visits() - initial_visits, APPENDS);
    assert!(historical_full_scan_visits > APPENDS * 500);
    assert_matches_full_capture(&runtime, &cursor, &context);
    println!(
        "{{\"schema\":\"cindx.agent-runtime-snapshot-scaling.v1\",\"appends\":{APPENDS},\"incremental_message_visits\":{},\"historical_full_scan_visits\":{historical_full_scan_visits}}}",
        cursor.message_visits() - initial_visits
    );
}
