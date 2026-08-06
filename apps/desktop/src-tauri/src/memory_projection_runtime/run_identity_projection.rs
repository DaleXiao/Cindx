use agent_core::{
    AgentRunIdentity, AgentRunLineage, Event, TaskId, AGENT_RUN_ID_METADATA_KEY,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use agent_storage::{SqliteStore, StorageError};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum MemoryRunScope {
    Logical {
        run_id: String,
        session_id: Option<String>,
    },
    Physical {
        run_id: String,
        session_id: Option<String>,
    },
}

impl MemoryRunScope {
    fn indexed_metadata(&self) -> (&'static str, &str) {
        match self {
            Self::Logical { run_id, .. } => (LOGICAL_AGENT_RUN_ID_METADATA_KEY, run_id),
            Self::Physical { run_id, .. } => (AGENT_RUN_ID_METADATA_KEY, run_id),
        }
    }

    pub(super) fn session_id(&self) -> Option<&str> {
        match self {
            Self::Logical { session_id, .. } | Self::Physical { session_id, .. } => {
                session_id.as_deref()
            }
        }
    }
}

pub(super) type ScopedMemoryRunEvents<'a> = BTreeMap<MemoryRunScope, Vec<&'a Event>>;
pub(super) type BridgedMemoryRunEvents =
    BTreeMap<Option<String>, BTreeMap<MemoryRunScope, Vec<Event>>>;

pub(super) fn memory_run_scope(
    event: &Event,
    lineage: Option<&AgentRunLineage>,
) -> Option<MemoryRunScope> {
    let session_id = event.metadata.get("session_id").cloned();
    if let Some(lineage) = lineage {
        if let Ok(Some(logical_run_id)) = lineage.logical_run_id_for_event(event) {
            return Some(MemoryRunScope::Logical {
                run_id: logical_run_id.to_string(),
                session_id,
            });
        }
    }
    event
        .metadata
        .get(AGENT_RUN_ID_METADATA_KEY)
        .filter(|run_id| !run_id.is_empty())
        .map(|run_id| MemoryRunScope::Physical {
            run_id: run_id.clone(),
            session_id,
        })
}

pub(super) fn group_memory_run_events<'a>(
    events: &'a [Event],
    project_id: &str,
    lineage: Option<&AgentRunLineage>,
) -> ScopedMemoryRunEvents<'a> {
    events.iter().fold(
        BTreeMap::<MemoryRunScope, Vec<&Event>>::new(),
        |mut runs, event| {
            if event.metadata.get("project_id").map(String::as_str) == Some(project_id) {
                if let Some(scope) = memory_run_scope(event, lineage) {
                    runs.entry(scope).or_default().push(event);
                }
            }
            runs
        },
    )
}

pub(super) fn select_memory_run_events(
    store: &mut SqliteStore,
    task_id: &TaskId,
    project_id: &str,
    scope: &MemoryRunScope,
    scoped_run_events: Option<&ScopedMemoryRunEvents<'_>>,
    bridged_run_events: &mut BridgedMemoryRunEvents,
) -> Result<Vec<Event>, StorageError> {
    let mut events = match scoped_run_events {
        Some(runs) => runs
            .get(scope)
            .map(|events| events.iter().map(|event| (**event).clone()).collect())
            .unwrap_or_default(),
        None => {
            let (metadata_key, run_id) = scope.indexed_metadata();
            store.list_by_task_and_metadata(task_id, metadata_key, run_id)?
        }
    };
    events.retain(|candidate| {
        candidate.metadata.get("project_id").map(String::as_str) == Some(project_id)
            && candidate.metadata.get("session_id").map(String::as_str) == scope.session_id()
    });
    if scoped_run_events.is_none() && indexed_memory_run_requires_lineage_bridge(&events) {
        bridge_memory_run_events(
            store,
            task_id,
            project_id,
            scope,
            bridged_run_events,
            &mut events,
        )?;
    }
    if matches!(scope, MemoryRunScope::Logical { .. })
        && AgentRunLineage::from_events(&events).is_err()
    {
        events.clear();
    }
    Ok(events)
}

fn indexed_memory_run_requires_lineage_bridge(events: &[Event]) -> bool {
    let attempt_run_ids = events
        .iter()
        .filter_map(|event| event.metadata.get(AGENT_RUN_ID_METADATA_KEY))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    events.iter().any(|event| {
        let Ok(Some(identity)) = AgentRunIdentity::from_metadata(&event.metadata) else {
            return event
                .metadata
                .contains_key(LOGICAL_AGENT_RUN_ID_METADATA_KEY);
        };
        identity
            .source_attempt_run_id()
            .is_some_and(|source_run_id| !attempt_run_ids.contains(source_run_id))
    })
}

fn bridge_memory_run_events(
    store: &mut SqliteStore,
    task_id: &TaskId,
    project_id: &str,
    scope: &MemoryRunScope,
    bridged_run_events: &mut BridgedMemoryRunEvents,
    events: &mut Vec<Event>,
) -> Result<(), StorageError> {
    let session_id = scope.session_id().map(str::to_string);
    if !bridged_run_events.contains_key(&session_id) {
        let scope_events = match session_id.as_deref() {
            Some(session_id) => {
                store.list_by_task_and_metadata(task_id, "session_id", session_id)?
            }
            None => store.list_by_task_and_metadata(task_id, "project_id", project_id)?,
        };
        let scope_events = scope_events
            .into_iter()
            .filter(|candidate| {
                candidate.metadata.get("project_id").map(String::as_str) == Some(project_id)
                    && candidate.metadata.get("session_id").map(String::as_str)
                        == session_id.as_deref()
            })
            .collect::<Vec<_>>();
        let runs = AgentRunLineage::from_events(&scope_events)
            .ok()
            .map(|full_lineage| {
                scope_events.into_iter().fold(
                    BTreeMap::<MemoryRunScope, Vec<Event>>::new(),
                    |mut runs, event| {
                        if let Some(scope) = memory_run_scope(&event, Some(&full_lineage)) {
                            runs.entry(scope).or_default().push(event);
                        }
                        runs
                    },
                )
            })
            .unwrap_or_default();
        bridged_run_events.insert(session_id.clone(), runs);
    }
    let runs = bridged_run_events
        .get(&session_id)
        .expect("memory lineage bridge cache should be initialized");
    if let Some(bridged) = runs.get(scope).filter(|events| !events.is_empty()) {
        *events = bridged.clone();
    }
    Ok(())
}
