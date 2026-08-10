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
use agent_application::insert_runtime_message_display_prompt;
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
    ResolvedNoop {
        epoch: u64,
        retained_strategy_context: Option<Metadata>,
    },
    Applied(AppliedAgentSteer),
    Stopped(RunStopReason),
}

#[derive(Debug)]
struct CommittedSteerBatch {
    acknowledged_queue_ids: Vec<String>,
    latest: Option<AppliedAgentSteer>,
    snapshot_context: Option<Metadata>,
    retained_strategy_context: Option<Metadata>,
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

        let (model_prompt, metadata, created_at_ms, already_persisted) = if let Some(event) =
            durable_event
        {
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
            ]
            .into_iter()
            .collect::<Metadata>();
            insert_runtime_message_display_prompt(&mut metadata, &display_prompt, &model_prompt);
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
        retained_strategy_context: None,
    })
}

fn persist_pending_agent_steer_batch(
    store: &mut agent_storage::SqliteStore,
    workspace_root: &Path,
    runtime: &mut AgentLoopState,
    run_context: &Metadata,
    pending: &[RunSteer],
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
    retain_strategy_on_noop: bool,
) -> Result<CommittedSteerBatch, StorageError> {
    let task_id = runtime.task_id.clone();
    let mut transaction = AgentLoopAppendTransaction::begin(runtime);
    let previous_message_count = transaction.original_message_count();
    let mut committed_cursor = None;
    let committed = store.with_immediate_transaction(|store| {
        let mut committed = transaction.with_append_only_mutation(|runtime| {
            commit_pending_steers(store, workspace_root, runtime, run_context, pending)
        })?;
        if committed.acknowledged_queue_ids.len() != pending.len() {
            return Err(steer_storage_error(format!(
                "steer batch acknowledged {} of {} pending messages",
                committed.acknowledged_queue_ids.len(),
                pending.len()
            )));
        }
        if retain_strategy_on_noop && committed.latest.is_none() {
            let epoch = pending
                .last()
                .expect("a committed steer batch cannot be empty")
                .epoch;
            committed.retained_strategy_context =
                crate::agent_strategy_receipt_runtime::persist_retained_strategy_decision(
                    store,
                    &task_id,
                    run_context,
                    epoch,
                )?;
        }
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
    transaction.commit();
    if let Some(next_cursor) = committed_cursor {
        *snapshot_cursor = next_cursor;
    }
    Ok(committed)
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
        false,
    )
}

pub(crate) fn apply_pending_agent_steers_with_cursor(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut AgentLoopState,
    run_context: &Metadata,
    cancellation: &AgentRunControl,
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
    retain_strategy_on_noop: bool,
) -> Result<AgentSteerApplication, String> {
    let committed = cancellation
        .commit_pending_steers_with_applied_objective(
            |pending| {
                let mut store = state.store.lock().map_err(|error| {
                    steer_storage_error(format!("store lock poisoned: {error}"))
                })?;
                persist_pending_agent_steer_batch(
                    &mut store,
                    workspace_root,
                    runtime,
                    run_context,
                    pending,
                    snapshot_cursor,
                    retain_strategy_on_noop,
                )
            },
            |committed| committed.latest.is_some(),
        )
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
                Ok(AgentSteerApplication::ResolvedNoop {
                    epoch,
                    retained_strategy_context: committed.retained_strategy_context,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_persistence::append_event;
    use crate::runtime_constants::AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE;
    use agent_application::AgentStrategyDecisionReceipt;
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
    fn durable_applied_steer_opens_the_control_objective_segment() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = test_run_context();
        append_queue_action(&mut store, &run_context, "enqueue", "queue-segment");
        let mut runtime = start_agent_loop(
            phase16_task_id(),
            "Original objective",
            AgentRuntimeConfig::default(),
        );
        let mut budget = agent_runtime::RunBudget::for_effort("fast");
        budget.initial_model_calls = 1;
        budget.max_model_calls = 3;
        let control = AgentRunControl::with_budget(budget);
        assert_eq!(control.begin_model_call("initial"), Ok(1));
        control.finish_model_call();
        assert_eq!(control.request_steer("queue-segment"), Ok(true));

        let committed = control
            .commit_pending_steers_with_applied_objective(
                |pending| {
                    store.with_immediate_transaction(|transaction| {
                        commit_pending_steers(
                            transaction,
                            Path::new("."),
                            &mut runtime,
                            &run_context,
                            pending,
                        )
                    })
                },
                |committed| committed.latest.is_some(),
            )
            .expect("durable steer should commit");

        assert!(matches!(committed, RunSteerBatchCommit::Committed { .. }));
        assert_eq!(control.progress().model_call_limit, 2);
        assert_eq!(control.begin_model_call_at(1, "steered"), Ok(Some(2)));
        control.finish_model_call();
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
        let mut budget = agent_runtime::RunBudget::for_effort("fast");
        budget.initial_model_calls = 1;
        budget.max_model_calls = 3;
        let control = AgentRunControl::with_budget(budget);
        assert_eq!(control.begin_model_call("initial"), Ok(1));
        control.finish_model_call();
        assert_eq!(control.request_steer("queue-deleted"), Ok(true));
        let committed = match control
            .commit_pending_steers_with_applied_objective(
                |pending| {
                    store.with_immediate_transaction(|transaction| {
                        commit_pending_steers(
                            transaction,
                            Path::new("."),
                            &mut runtime,
                            &run_context,
                            pending,
                        )
                    })
                },
                |committed| committed.latest.is_some(),
            )
            .expect("deleted steer should resolve")
        {
            RunSteerBatchCommit::Committed { value, .. } => value,
            outcome => panic!("deleted steer should commit as a no-op: {outcome:?}"),
        };
        assert_eq!(
            committed.acknowledged_queue_ids,
            vec!["queue-deleted".to_string()]
        );
        assert!(committed.latest.is_none());
        assert_eq!(runtime.messages.len(), 1);
        assert_eq!(control.progress().model_call_limit, 1);
    }

    #[test]
    fn agent_strategy_lifecycle_contract_preparation_noop_replans_without_precommitting_a_decision(
    ) {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let mut run_context = test_run_context();
        run_context.insert("steer_epoch".to_string(), "0".to_string());
        let initial_plan_sha256 = "a".repeat(64);
        run_context.insert(
            "execution_plan_semantic_sha256".to_string(),
            initial_plan_sha256.clone(),
        );
        let initial_receipt = AgentStrategyDecisionReceipt::new(
            &phase16_task_id(),
            &run_context,
            &initial_plan_sha256,
        )
        .expect("initial receipt should build");
        initial_receipt
            .insert_into(&mut run_context)
            .expect("initial receipt should attach");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent run decision selected",
            run_context.clone(),
        )
        .expect("initial decision should persist");
        append_queue_action(&mut store, &run_context, "enqueue", "queue-preparation-noop");
        append_queue_action(&mut store, &run_context, "delete", "queue-preparation-noop");

        let control = AgentRunControl::new("pro");
        assert_eq!(control.request_steer("queue-preparation-noop"), Ok(true));
        let mut runtime = start_agent_loop(
            phase16_task_id(),
            "Original objective",
            AgentRuntimeConfig::default(),
        );
        let mut snapshot_cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &run_context);
        let committed = control
            .commit_pending_steers_with_applied_objective(
                |pending| {
                    persist_pending_agent_steer_batch(
                        &mut store,
                        Path::new("."),
                        &mut runtime,
                        &run_context,
                        pending,
                        &mut snapshot_cursor,
                        false,
                    )
                },
                |committed| committed.latest.is_some(),
            )
            .expect("deleted preparation steer should resolve");
        let RunSteerBatchCommit::Committed { value, .. } = committed else {
            panic!("deleted preparation steer should commit as a no-op")
        };
        assert!(value.retained_strategy_context.is_none());
        assert_eq!(control.steer_epoch(), 1);

        let decisions = store
            .list_by_task(&phase16_task_id())
            .expect("decisions should load")
            .into_iter()
            .filter_map(|event| AgentStrategyDecisionReceipt::from_decision_event(&event).ok())
            .collect::<Vec<_>>();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].steer_epoch(), 0);

        let mut replanned_context = test_run_context();
        replanned_context.insert("steer_epoch".to_string(), "1".to_string());
        let replanned_sha256 = "b".repeat(64);
        let replanned_receipt = AgentStrategyDecisionReceipt::new(
            &phase16_task_id(),
            &replanned_context,
            &replanned_sha256,
        )
        .expect("replanned receipt should build");
        replanned_receipt
            .insert_into(&mut replanned_context)
            .expect("replanned receipt should attach");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent run decision selected",
            replanned_context,
        )
        .expect("a different plan should persist for the advanced epoch");
        let decisions = store
            .list_by_task(&phase16_task_id())
            .expect("replanned decisions should load")
            .into_iter()
            .filter_map(|event| AgentStrategyDecisionReceipt::from_decision_event(&event).ok())
            .collect::<Vec<_>>();
        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().any(|receipt| {
            receipt.steer_epoch() == 1 && receipt.plan_sha256() == replanned_sha256
        }));
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

    #[test]
    fn steer_snapshot_failure_rolls_back_and_retries_exactly_once() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = test_run_context();
        append_queue_action(&mut store, &run_context, "enqueue", "queue-retry");
        let control = AgentRunControl::new("pro");
        assert_eq!(control.request_steer("queue-retry"), Ok(true));
        let pending_before = control.pending_steers_snapshot();
        let mut runtime = start_agent_loop(
            phase16_task_id(),
            "Original objective",
            AgentRuntimeConfig::default(),
        );
        let runtime_before = runtime.clone();
        let mut snapshot_cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &run_context);
        let cursor_visits_before = snapshot_cursor.message_visits();
        store
            .execute_batch_for_testing(
                "
                create trigger fail_steer_runtime_snapshot
                before insert on read_models
                when new.namespace = 'agent-runtime-snapshot-v1'
                begin
                  select raise(abort, 'injected steer snapshot failure');
                end;
                ",
            )
            .expect("failure trigger should install");

        let failed = control.commit_pending_steers_with(|pending| {
            persist_pending_agent_steer_batch(
                &mut store,
                Path::new("."),
                &mut runtime,
                &run_context,
                pending,
                &mut snapshot_cursor,
                true,
            )
        });

        assert!(failed.is_err());
        assert_eq!(runtime, runtime_before);
        assert_eq!(snapshot_cursor.message_visits(), cursor_visits_before);
        assert_eq!(control.pending_steers_snapshot(), pending_before);
        let failed_events = store
            .list_by_task(&phase16_task_id())
            .expect("events should load");
        assert_eq!(
            failed_events
                .iter()
                .filter(|event| event_matches_steer(event, "queue-retry", "run-a"))
                .count(),
            0
        );
        assert_eq!(
            failed_events
                .iter()
                .filter(|event| {
                    event.metadata.get("queue_id").map(String::as_str) == Some("queue-retry")
                        && event.metadata.get("queue_action").map(String::as_str) == Some("start")
                })
                .count(),
            0
        );
        assert!(store
            .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
            .expect("runtime checkpoint lookup should succeed")
            .is_none());

        store
            .execute_batch_for_testing("drop trigger fail_steer_runtime_snapshot;")
            .expect("failure trigger should uninstall");
        let retried = control
            .commit_pending_steers_with(|pending| {
                persist_pending_agent_steer_batch(
                    &mut store,
                    Path::new("."),
                    &mut runtime,
                    &run_context,
                    pending,
                    &mut snapshot_cursor,
                    true,
                )
            })
            .expect("steer retry should persist");

        assert!(matches!(retried, RunSteerBatchCommit::Committed { .. }));
        assert!(control.pending_steers_snapshot().is_empty());
        assert_eq!(runtime.messages.len(), runtime_before.messages.len() + 1);
        assert_eq!(snapshot_cursor.message_visits(), cursor_visits_before + 1);
        let committed_events = store
            .list_by_task(&phase16_task_id())
            .expect("events should load");
        assert_eq!(
            committed_events
                .iter()
                .filter(|event| event_matches_steer(event, "queue-retry", "run-a"))
                .count(),
            1
        );
        assert_eq!(
            committed_events
                .iter()
                .filter(|event| {
                    event.metadata.get("queue_id").map(String::as_str) == Some("queue-retry")
                        && event.metadata.get("queue_action").map(String::as_str) == Some("start")
                })
                .count(),
            1
        );
        assert!(store
            .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
            .expect("runtime checkpoint lookup should succeed")
            .is_some());
        assert_eq!(
            control
                .commit_pending_steers_with(|_| -> Result<(), StorageError> {
                    panic!("drained steer must not run twice")
                })
                .expect("empty steer batch should resolve"),
            RunSteerBatchCommit::NoPending
        );
    }
}
