use crate::agent_commands::{
    add_attachment_metadata, prompt_with_attachments, validate_agent_attachments,
};
use crate::agent_query_commands::append_agent_queue_event;
use crate::agent_runtime_snapshot::persist_prepared_agent_runtime_snapshot;
use crate::agent_runtime_snapshot_cursor::AgentRuntimeSnapshotCursor;
use crate::app_state::AppState;
use crate::event_persistence::persist_new_runtime_messages;
#[cfg(test)]
use crate::project_session_persistence::metadata_with_context;
use crate::queue_service::pending_queued_agent_messages;
use crate::runtime_values::phase16_task_id;
use crate::tool_execution::runtime_message_from_event;
#[cfg(test)]
use agent_core::MessageRole;
use agent_core::{Event, EventKind, Message, Metadata};
use agent_runtime::{
    AgentKernel, AgentLoopAppendTransaction, AgentLoopState, AgentRunControl, RunSteer,
    RunSteerBatchCommit, RunStopReason,
};
use agent_storage::StorageError;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppliedAgentSteer {
    pub(crate) prompt: String,
    pub(crate) epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentSteerApplication {
    NoPending,
    ResolvedNoop { epoch: u64 },
    Applied(AppliedAgentSteer),
    Stopped(RunStopReason),
}

#[derive(Debug)]
struct CommittedSteerBatch {
    acknowledged_queue_ids: Vec<String>,
    latest: Option<AppliedAgentSteer>,
    snapshot_context: Option<Metadata>,
}

fn steer_storage_error(error: impl Into<String>) -> StorageError {
    StorageError::new(error.into())
}

fn event_matches_steer(event: &Event, queue_id: &str, run_id: &str) -> bool {
    event.kind == EventKind::MessageAdded
        && event.metadata.get("role").map(String::as_str) == Some("user")
        && event.metadata.get("queue_mode").map(String::as_str) == Some("steer")
        && event.metadata.get("queue_id").map(String::as_str) == Some(queue_id)
        && event.metadata.get("agent_run_id").map(String::as_str) == Some(run_id)
}

fn latest_queue_action<'a>(events: &'a [Event], queue_id: &str) -> Option<&'a str> {
    events.iter().rev().find_map(|event| {
        (event.metadata.get("queue_id").map(String::as_str) == Some(queue_id))
            .then(|| event.metadata.get("queue_action").map(String::as_str))
            .flatten()
    })
}

fn message_has_queue_id(message: &Message, queue_id: &str) -> bool {
    message.metadata.get("queue_id").map(String::as_str) == Some(queue_id)
}

fn commit_pending_steers(
    store: &mut agent_storage::SqliteStore,
    workspace_root: &Path,
    runtime: &mut AgentLoopState,
    run_context: &Metadata,
    pending: &[RunSteer],
) -> Result<CommittedSteerBatch, StorageError> {
    let session_id = run_context
        .get("session_id")
        .ok_or_else(|| steer_storage_error("queued steer has no session context"))?;
    let run_id = run_context
        .get("agent_run_id")
        .ok_or_else(|| steer_storage_error("queued steer has no run context"))?;
    let queue_ids = pending
        .iter()
        .map(|steer| steer.queue_id.clone())
        .collect::<Vec<_>>();
    let events = store.list_by_task_and_metadata_values_in_scope(
        &phase16_task_id(),
        "queue_id",
        &queue_ids,
        "session_id",
        session_id,
    )?;
    let queued = pending_queued_agent_messages(&events, session_id);
    let mut acknowledged_queue_ids = Vec::with_capacity(pending.len());
    let mut latest = None;
    let mut snapshot_context = run_context.clone();

    for pending_steer in pending {
        let queue_id = pending_steer.queue_id.as_str();
        let action = latest_queue_action(&events, queue_id);
        if action == Some("delete") {
            acknowledged_queue_ids.push(queue_id.to_string());
            continue;
        }

        let durable_event = events
            .iter()
            .rev()
            .find(|event| event_matches_steer(event, queue_id, run_id));
        let queued_message = queued.iter().find(|message| message.view.id == queue_id);
        if action == Some("start") && durable_event.is_none() {
            return Err(steer_storage_error(format!(
                "queued steer {queue_id} started without a durable user message"
            )));
        }

        let (model_prompt, metadata, created_at_ms, already_persisted) =
            if let Some(event) = durable_event {
                let message = runtime_message_from_event(event).ok_or_else(|| {
                    steer_storage_error(format!(
                        "durable steer {queue_id} could not be restored as a runtime message"
                    ))
                })?;
                (message.content, message.metadata, None, true)
            } else if let Some(queued_message) = queued_message {
                let attachments = validate_agent_attachments(
                    workspace_root,
                    queued_message.payload.attachments.clone(),
                )
                .map_err(steer_storage_error)?;
                let display_prompt = queued_message.payload.prompt.clone();
                let model_prompt = prompt_with_attachments(&display_prompt, &attachments);
                let mut metadata = [
                    ("queue_id".to_string(), queue_id.to_string()),
                    ("queue_mode".to_string(), "steer".to_string()),
                    ("display_content".to_string(), display_prompt),
                    ("model_content".to_string(), model_prompt.clone()),
                ]
                .into_iter()
                .collect::<Metadata>();
                add_attachment_metadata(&mut metadata, &attachments);
                (
                    model_prompt,
                    metadata,
                    Some(queued_message.view.created_at_ms),
                    false,
                )
            } else {
                return Err(steer_storage_error(format!(
                    "queued steer {queue_id} disappeared before it could be applied"
                )));
            };

        let mut steer_context = run_context.clone();
        steer_context.insert("steer_epoch".to_string(), pending_steer.epoch.to_string());
        if !runtime
            .messages
            .iter()
            .any(|message| message_has_queue_id(message, queue_id))
        {
            let previous_message_count = runtime.messages.len();
            AgentKernel::new(runtime, &[]).apply_steer(model_prompt.clone(), metadata);
            if !already_persisted {
                persist_new_runtime_messages(
                    store,
                    &runtime.task_id,
                    &runtime.messages,
                    previous_message_count,
                    &steer_context,
                )?;
            }
        }
        if let Some(created_at_ms) = created_at_ms {
            append_agent_queue_event(
                store,
                &steer_context,
                "start",
                queue_id,
                "steer",
                created_at_ms,
                None,
            )
            .map_err(steer_storage_error)?;
        }
        snapshot_context = steer_context;
        acknowledged_queue_ids.push(queue_id.to_string());
        latest = Some(AppliedAgentSteer {
            prompt: model_prompt,
            epoch: pending_steer.epoch,
        });
    }

    let snapshot_context = latest.is_some().then_some(snapshot_context);
    Ok(CommittedSteerBatch {
        acknowledged_queue_ids,
        latest,
        snapshot_context,
    })
}

pub(crate) fn apply_pending_agent_steers(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut AgentLoopState,
    run_context: &Metadata,
    cancellation: &AgentRunControl,
) -> Result<AgentSteerApplication, String> {
    let mut snapshot_cursor = AgentRuntimeSnapshotCursor::rebuild(runtime, run_context);
    apply_pending_agent_steers_with_cursor(
        state,
        workspace_root,
        runtime,
        run_context,
        cancellation,
        &mut snapshot_cursor,
    )
}

pub(crate) fn apply_pending_agent_steers_with_cursor(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut AgentLoopState,
    run_context: &Metadata,
    cancellation: &AgentRunControl,
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
) -> Result<AgentSteerApplication, String> {
    let committed = cancellation
        .commit_pending_steers_with(|pending| {
            let transaction = AgentLoopAppendTransaction::begin(runtime);
            let previous_message_count = transaction.original_message_count();
            let mut transaction = transaction;
            let mut store = state
                .store
                .lock()
                .map_err(|error| steer_storage_error(format!("store lock poisoned: {error}")))?;
            let mut committed_cursor = None;
            let committed = store.with_immediate_transaction(|store| {
                let committed = commit_pending_steers(
                    store,
                    workspace_root,
                    transaction.state_mut(),
                    run_context,
                    pending,
                )?;
                if let Some(snapshot_context) = committed.snapshot_context.as_ref() {
                    let (prepared_snapshot, next_cursor) = snapshot_cursor.prepare_after_append(
                        transaction.state(),
                        previous_message_count,
                        snapshot_context,
                    );
                    persist_prepared_agent_runtime_snapshot(store, &prepared_snapshot)
                        .map_err(steer_storage_error)?;
                    committed_cursor = Some(next_cursor);
                }
                Ok(committed)
            })?;
            debug_assert_eq!(committed.acknowledged_queue_ids.len(), pending.len());
            transaction.commit();
            if let Some(next_cursor) = committed_cursor {
                *snapshot_cursor = next_cursor;
            }
            Ok::<_, StorageError>(committed)
        })
        .map_err(|error| error.to_string())?;

    match committed {
        RunSteerBatchCommit::NoPending => Ok(AgentSteerApplication::NoPending),
        RunSteerBatchCommit::Stopped(reason) => Ok(AgentSteerApplication::Stopped(reason)),
        RunSteerBatchCommit::Committed {
            value: committed,
            steers,
        } => {
            if let Some(latest) = committed.latest {
                cancellation.record_checkpoint_at(
                    latest.epoch,
                    "steering",
                    "User guidance applied",
                    &latest.prompt,
                );
                Ok(AgentSteerApplication::Applied(latest))
            } else {
                let epoch = steers
                    .last()
                    .expect("a committed steer batch cannot be empty")
                    .epoch;
                Ok(AgentSteerApplication::ResolvedNoop { epoch })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_persistence::append_event;
    use agent_core::EventKind;
    use agent_runtime::{start_agent_loop, AgentRuntimeConfig};
    use agent_storage::{EventStore, SqliteStore};

    fn test_run_context() -> Metadata {
        [
            ("project_id".to_string(), "project-a".to_string()),
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
        ]
        .into_iter()
        .collect()
    }

    fn append_queue_action(
        store: &mut SqliteStore,
        run_context: &Metadata,
        action: &str,
        queue_id: &str,
    ) {
        let payload = serde_json::json!({
            "prompt": "Use the revised objective",
            "attachments": [],
            "effort": "auto",
            "currentTime": "test time"
        });
        let metadata = metadata_with_context(
            [
                ("queue_action".to_string(), action.to_string()),
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_mode".to_string(), "steer".to_string()),
                ("queue_created_at_ms".to_string(), "1".to_string()),
                ("queue_payload".to_string(), payload.to_string()),
            ]
            .into_iter()
            .collect(),
            run_context,
        );
        append_event(
            store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            format!("queue {action}"),
            metadata,
        )
        .expect("queue action should append");
    }

    #[test]
    fn durable_steer_commit_recovers_after_commit_before_ack_without_duplicates() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = test_run_context();
        append_queue_action(&mut store, &run_context, "enqueue", "queue-a");
        let pending = vec![RunSteer {
            queue_id: "queue-a".to_string(),
            epoch: 1,
        }];
        let mut runtime = start_agent_loop(
            phase16_task_id(),
            "Original objective",
            AgentRuntimeConfig::default(),
        );
        let committed = store
            .with_immediate_transaction(|transaction| {
                commit_pending_steers(
                    transaction,
                    Path::new("."),
                    &mut runtime,
                    &run_context,
                    &pending,
                )
            })
            .expect("first commit should succeed");
        assert_eq!(
            committed.latest.as_ref().map(|steer| steer.prompt.as_str()),
            Some("Use the revised objective")
        );
        assert_eq!(runtime.messages.len(), 2);

        let mut recovered_runtime = start_agent_loop(
            phase16_task_id(),
            "Original objective",
            AgentRuntimeConfig::default(),
        );
        let recovered = store
            .with_immediate_transaction(|transaction| {
                commit_pending_steers(
                    transaction,
                    Path::new("."),
                    &mut recovered_runtime,
                    &run_context,
                    &pending,
                )
            })
            .expect("recovery commit should succeed");
        assert_eq!(recovered_runtime.messages.len(), 2);
        assert_eq!(recovered.latest.map(|steer| steer.epoch), Some(1));

        let events = store
            .list_by_task(&phase16_task_id())
            .expect("events should load");
        assert_eq!(
            events
                .iter()
                .filter(|event| event_matches_steer(event, "queue-a", "run-a"))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.metadata.get("queue_id").map(String::as_str) == Some("queue-a")
                        && event.metadata.get("queue_action").map(String::as_str) == Some("start")
                })
                .count(),
            1
        );
    }

    #[test]
    fn deleted_pending_steer_is_acknowledged_without_execution() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = test_run_context();
        append_queue_action(&mut store, &run_context, "enqueue", "queue-deleted");
        append_queue_action(&mut store, &run_context, "delete", "queue-deleted");
        let mut runtime = start_agent_loop(
            phase16_task_id(),
            "Original objective",
            AgentRuntimeConfig::default(),
        );
        let committed = store
            .with_immediate_transaction(|transaction| {
                commit_pending_steers(
                    transaction,
                    Path::new("."),
                    &mut runtime,
                    &run_context,
                    &[RunSteer {
                        queue_id: "queue-deleted".to_string(),
                        epoch: 1,
                    }],
                )
            })
            .expect("deleted steer should resolve");
        assert_eq!(
            committed.acknowledged_queue_ids,
            vec!["queue-deleted".to_string()]
        );
        assert!(committed.latest.is_none());
        assert_eq!(runtime.messages.len(), 1);
    }

    #[test]
    fn steer_persists_synthetic_tool_closure_before_the_user_instruction() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = test_run_context();
        append_queue_action(&mut store, &run_context, "enqueue", "queue-tools");
        let mut runtime = start_agent_loop(
            phase16_task_id(),
            "Original objective",
            AgentRuntimeConfig::default(),
        );
        runtime.messages.push(Message {
            role: MessageRole::Assistant,
            content: String::new(),
            metadata: [("tool_call_ids".to_string(), "call-a,call-b".to_string())]
                .into_iter()
                .collect(),
        });
        runtime.messages.push(Message {
            role: MessageRole::Tool,
            content: "call-a completed".to_string(),
            metadata: [("tool_call_id".to_string(), "call-a".to_string())]
                .into_iter()
                .collect(),
        });

        store
            .with_immediate_transaction(|transaction| {
                commit_pending_steers(
                    transaction,
                    Path::new("."),
                    &mut runtime,
                    &run_context,
                    &[RunSteer {
                        queue_id: "queue-tools".to_string(),
                        epoch: 1,
                    }],
                )
            })
            .expect("steer commit should succeed");

        assert_eq!(runtime.messages.len(), 5);
        assert_eq!(runtime.messages[3].role, MessageRole::Tool);
        assert_eq!(
            runtime.messages[3]
                .metadata
                .get("tool_call_id")
                .map(String::as_str),
            Some("call-b")
        );
        assert_eq!(runtime.messages[4].role, MessageRole::User);
        let message_events = store
            .list_by_task(&phase16_task_id())
            .expect("events should load")
            .into_iter()
            .filter(|event| event.kind == EventKind::MessageAdded)
            .collect::<Vec<_>>();
        assert_eq!(message_events.len(), 2);
        assert_eq!(
            message_events[0].metadata.get("role").map(String::as_str),
            Some("tool")
        );
        assert_eq!(
            message_events[0]
                .metadata
                .get("tool_call_id")
                .map(String::as_str),
            Some("call-b")
        );
        assert_eq!(
            message_events[1].metadata.get("role").map(String::as_str),
            Some("user")
        );
        assert_eq!(
            message_events[1]
                .metadata
                .get("queue_id")
                .map(String::as_str),
            Some("queue-tools")
        );
    }
}
