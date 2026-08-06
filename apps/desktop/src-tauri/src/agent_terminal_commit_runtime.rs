use crate::{agent_read_model::agent_state_for_session, view_models::AgentState};
use agent_application::AgentRunEvent;
use agent_core::{agent_run_id, Metadata, TaskId};
use agent_storage::{SqliteStore, StorageError};
use orchestrator::sha256_hex;

const AGENT_TERMINAL_COMMIT_SCHEMA: &str = "cindx.agent.terminal-commit.v1";
const AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY: &str = "agent_terminal_commit_schema";
const AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY: &str = "agent_terminal_commit_key";
const AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY: &str = "agent_terminal_commit_steer_epoch";
const AGENT_TERMINAL_COMMIT_HASH_DOMAIN: &str = "cindx.agent.terminal-commit.v1\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentTerminalCommitIdentity {
    key: String,
    agent_run_id: String,
    steer_epoch: u64,
}

impl AgentTerminalCommitIdentity {
    fn new(
        task_id: &TaskId,
        run_context: &Metadata,
        steer_epoch: u64,
    ) -> Result<Self, StorageError> {
        let agent_run_id = agent_run_id(run_context)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| StorageError::new("terminal commit requires a physical agent_run_id"))?;
        let session_id = run_context
            .get("session_id")
            .map(String::as_str)
            .unwrap_or_default();
        let canonical = serde_json::to_vec(&(
            AGENT_TERMINAL_COMMIT_HASH_DOMAIN,
            task_id.0.as_str(),
            session_id,
            agent_run_id,
            steer_epoch,
        ))
        .map_err(|error| {
            StorageError::new(format!(
                "failed to encode terminal commit identity: {error}"
            ))
        })?;
        Ok(Self {
            key: sha256_hex(&canonical),
            agent_run_id: agent_run_id.to_string(),
            steer_epoch,
        })
    }

    pub(crate) fn metadata(&self) -> Metadata {
        [
            (
                AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY.to_string(),
                AGENT_TERMINAL_COMMIT_SCHEMA.to_string(),
            ),
            (
                AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY.to_string(),
                self.key.clone(),
            ),
            (
                AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY.to_string(),
                self.steer_epoch.to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }
}

#[derive(Debug)]
pub(crate) struct PersistedAgentTerminal {
    pub(crate) state: AgentState,
    pub(crate) inserted: bool,
}

pub(crate) fn persist_agent_terminal_once(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    steer_epoch: u64,
    writer: impl FnOnce(
        &mut SqliteStore,
        &AgentTerminalCommitIdentity,
    ) -> Result<AgentState, StorageError>,
) -> Result<PersistedAgentTerminal, StorageError> {
    let identity = AgentTerminalCommitIdentity::new(task_id, run_context, steer_epoch)?;
    let session_id = run_context.get("session_id").map(String::as_str);
    store.with_immediate_transaction(|store| {
        match terminal_commit_event_count(store, task_id, &identity)? {
            0 => {}
            1 => {
                return agent_state_for_session(store, None, session_id).map(|state| {
                    PersistedAgentTerminal {
                        state,
                        inserted: false,
                    }
                });
            }
            _ => {
                return Err(StorageError::new(
                    "terminal commit identity resolves to multiple terminal events",
                ));
            }
        }

        let state = writer(store, &identity)?;
        if terminal_commit_event_count(store, task_id, &identity)? != 1 {
            return Err(StorageError::new(
                "terminal writer must persist exactly one matching terminal event",
            ));
        }
        Ok(PersistedAgentTerminal {
            state,
            inserted: true,
        })
    })
}

fn terminal_commit_event_count(
    store: &SqliteStore,
    task_id: &TaskId,
    identity: &AgentTerminalCommitIdentity,
) -> Result<usize, StorageError> {
    let events =
        store.list_by_task_and_metadata(task_id, "agent_run_id", &identity.agent_run_id)?;
    let mut count = 0usize;
    for event in events.iter().filter(|event| {
        event
            .metadata
            .get(AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY)
            .map(String::as_str)
            == Some(identity.key.as_str())
    }) {
        if event
            .metadata
            .get(AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY)
            .map(String::as_str)
            != Some(AGENT_TERMINAL_COMMIT_SCHEMA)
            || event
                .metadata
                .get(AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY)
                .and_then(|value| value.parse::<u64>().ok())
                != Some(identity.steer_epoch)
        {
            return Err(StorageError::new(
                "persisted terminal commit identity is malformed",
            ));
        }
        let terminal = AgentRunEvent::try_from_event(event)
            .map_err(|error| StorageError::new(error.to_string()))?
            .is_some_and(|event| event.status().is_terminal());
        if !terminal {
            return Err(StorageError::new(
                "terminal commit identity is attached to a nonterminal event",
            ));
        }
        count = count.saturating_add(1);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event_persistence::{append_event, append_message_event_with_metadata},
        project_session_persistence::metadata_with_context,
        runtime_values::phase16_task_id,
    };
    use agent_core::{EventKind, MessageRole};
    use agent_runtime::{AgentRunControl, RunEpochLeaseOutcome, RunTerminalCommit};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn run_context() -> Metadata {
        [
            ("project_id".to_string(), "project-terminal".to_string()),
            ("session_id".to_string(), "session-terminal".to_string()),
            ("agent_run_id".to_string(), "run-terminal".to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
        ]
        .into_iter()
        .collect()
    }

    fn persist_completed_terminal(
        store: &mut SqliteStore,
        identity: &AgentTerminalCommitIdentity,
        context: &Metadata,
    ) -> Result<AgentState, StorageError> {
        append_message_event_with_metadata(
            store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "terminal answer",
            context.clone(),
        )?;
        append_event(
            store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task completed",
            metadata_with_context(identity.metadata(), context),
        )?;
        agent_state_for_session(store, None, Some("session-terminal"))
    }

    fn persist_failed_terminal(
        store: &mut SqliteStore,
        identity: &AgentTerminalCommitIdentity,
        context: &Metadata,
    ) -> Result<AgentState, StorageError> {
        append_event(
            store,
            &phase16_task_id(),
            EventKind::Error,
            "Agent task failed",
            metadata_with_context(identity.metadata(), context),
        )?;
        agent_state_for_session(
            store,
            Some("Finalizer terminal commit failed".to_string()),
            Some("session-terminal"),
        )
    }

    #[test]
    fn independent_run_controls_replay_one_durable_terminal_commit() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let context = run_context();
        let writes = AtomicUsize::new(0);

        let first_control = AgentRunControl::new("auto");
        let first_lease = match first_control.execution_epoch_lease() {
            RunEpochLeaseOutcome::Acquired(lease) => lease,
            outcome => panic!("first execution lease unavailable: {outcome:?}"),
        };
        let first = first_control
            .commit_terminal_result_with(first_lease, || {
                persist_agent_terminal_once(
                    &mut store,
                    &phase16_task_id(),
                    &context,
                    0,
                    |store, identity| {
                        writes.fetch_add(1, Ordering::SeqCst);
                        persist_completed_terminal(store, identity, &context)
                    },
                )
            })
            .expect("first terminal commit should persist");
        assert!(matches!(
            first,
            RunTerminalCommit::Committed(PersistedAgentTerminal { inserted: true, .. })
        ));

        let replay_control = AgentRunControl::new("auto");
        let replay_lease = match replay_control.execution_epoch_lease() {
            RunEpochLeaseOutcome::Acquired(lease) => lease,
            outcome => panic!("replay execution lease unavailable: {outcome:?}"),
        };
        let replay = replay_control
            .commit_terminal_result_with(replay_lease, || {
                persist_agent_terminal_once(
                    &mut store,
                    &phase16_task_id(),
                    &context,
                    0,
                    |store, identity| {
                        writes.fetch_add(1, Ordering::SeqCst);
                        persist_completed_terminal(store, identity, &context)
                    },
                )
            })
            .expect("durable replay should load the existing terminal state");
        assert!(matches!(
            replay,
            RunTerminalCommit::Committed(PersistedAgentTerminal {
                inserted: false,
                ..
            })
        ));
        assert_eq!(writes.load(Ordering::SeqCst), 1);

        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "agent_run_id", "run-terminal")
            .expect("terminal events should load");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == EventKind::MessageAdded)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event
                        .metadata
                        .contains_key(AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY)
                })
                .count(),
            1
        );
    }

    #[test]
    fn failed_terminal_transaction_rolls_back_before_a_clean_retry() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let context = run_context();
        let error = persist_agent_terminal_once(
            &mut store,
            &phase16_task_id(),
            &context,
            0,
            |store, identity| {
                persist_completed_terminal(store, identity, &context)?;
                Err(StorageError::new("injected terminal transaction failure"))
            },
        )
        .expect_err("injected writer failure should roll back");
        assert!(error
            .to_string()
            .contains("injected terminal transaction failure"));
        assert!(store
            .list_by_task_and_metadata(&phase16_task_id(), "agent_run_id", "run-terminal")
            .expect("rolled back events should load")
            .is_empty());

        let retry = persist_agent_terminal_once(
            &mut store,
            &phase16_task_id(),
            &context,
            0,
            |store, identity| persist_completed_terminal(store, identity, &context),
        )
        .expect("retry should commit from a clean transaction");
        assert!(retry.inserted);
        let identity = AgentTerminalCommitIdentity::new(&phase16_task_id(), &context, 0)
            .expect("terminal identity should be stable");
        assert_eq!(
            terminal_commit_event_count(&store, &phase16_task_id(), &identity)
                .expect("retry terminal event should load"),
            1
        );
    }

    #[test]
    fn failed_success_writer_can_commit_one_durable_failure_terminal() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let context = run_context();
        let control = AgentRunControl::new("auto");
        let lease = match control.execution_epoch_lease() {
            RunEpochLeaseOutcome::Acquired(lease) => lease,
            outcome => panic!("execution lease unavailable: {outcome:?}"),
        };

        let error = control
            .commit_terminal_result_with(lease, || {
                persist_agent_terminal_once(
                    &mut store,
                    &phase16_task_id(),
                    &context,
                    0,
                    |store, identity| {
                        persist_completed_terminal(store, identity, &context)?;
                        Err(StorageError::new("injected Finalizer commit failure"))
                    },
                )
            })
            .expect_err("failed success transaction should remain retryable");
        assert!(error
            .to_string()
            .contains("injected Finalizer commit failure"));

        let failure = control
            .commit_terminal_result_with(lease, || {
                persist_agent_terminal_once(
                    &mut store,
                    &phase16_task_id(),
                    &context,
                    0,
                    |store, identity| persist_failed_terminal(store, identity, &context),
                )
            })
            .expect("failure terminal should persist after rollback");
        assert!(matches!(
            failure,
            RunTerminalCommit::Committed(PersistedAgentTerminal { inserted: true, .. })
        ));

        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "agent_run_id", "run-terminal")
            .expect("terminal events should load");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == EventKind::Error)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == EventKind::MessageAdded)
                .count(),
            0
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event
                    .metadata
                    .contains_key(AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY))
                .count(),
            1
        );
    }
}
