//! Routing-telemetry read model: inert measurement plumbing kept alive by its
//! contract tests after the conductor learning path was removed.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};

use agent_application::AgentRunEvent;
use agent_core::{parse_policy, RoutingTelemetry, TaskClass};
use agent_core::{
    AgentRunIdentity, AgentRunLineage, Event, EventKind, LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use agent_storage::{EventStore, SqliteStore, StorageError};

use crate::learning_evidence_runtime::{
    learning_lineage_usage_from_metadata, routing_learning_evidence,
};
use crate::routing_learning_runtime::{routing_outcome_for_run, routing_quality_signals};
use crate::runtime_constants::{
    ROUTING_TELEMETRY_MAX_RUNS, ROUTING_TELEMETRY_READ_MODEL_KEY,
    ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
};
use crate::runtime_values::phase16_task_id;
use crate::view_models::{RoutingTelemetryEntry, RoutingTelemetryReadModel};

fn parse_task_class_label(value: &str) -> Option<TaskClass> {
    match value {
        "general" => Some(TaskClass::General),
        "coding" => Some(TaskClass::Coding),
        "research" => Some(TaskClass::Research),
        "retrieval" => Some(TaskClass::Retrieval),
        "browser" => Some(TaskClass::Browser),
        "computer" => Some(TaskClass::Computer),
        _ => None,
    }
}

fn grouped_agent_run_events(events: &[Event]) -> BTreeMap<String, Vec<&Event>> {
    let Ok(lineage) = AgentRunLineage::from_events(events) else {
        let physical_scopes = events.iter().fold(
            BTreeMap::<&str, BTreeSet<(&str, Option<&str>, Option<&str>)>>::new(),
            |mut scopes, event| {
                if let Some(run_id) = event
                    .metadata
                    .get("agent_run_id")
                    .filter(|run_id| !run_id.is_empty())
                {
                    scopes.entry(run_id).or_default().insert((
                        event.task_id.0.as_str(),
                        event.metadata.get("project_id").map(String::as_str),
                        event.metadata.get("session_id").map(String::as_str),
                    ));
                }
                scopes
            },
        );
        return events.iter().fold(BTreeMap::new(), |mut runs, event| {
            if let Some(run_id) = event
                .metadata
                .get("agent_run_id")
                .filter(|run_id| !run_id.is_empty())
                .filter(|run_id| {
                    physical_scopes
                        .get(run_id.as_str())
                        .is_some_and(|scopes| scopes.len() == 1)
                })
            {
                runs.entry(run_id.clone()).or_default().push(event);
            }
            runs
        });
    };
    events.iter().fold(BTreeMap::new(), |mut runs, event| {
        if let Ok(Some(logical_run_id)) = lineage.logical_run_id_for_event(event) {
            runs.entry(logical_run_id.to_string())
                .or_default()
                .push(event);
        }
        runs
    })
}

fn indexed_run_requires_lineage_bridge(events: &[Event]) -> bool {
    let attempt_run_ids = events
        .iter()
        .filter_map(|event| event.metadata.get("agent_run_id"))
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

fn is_agent_run_terminal(event: &Event) -> bool {
    AgentRunEvent::from_event(event).is_some_and(|event| event.status().is_terminal())
}

fn active_agent_run_latency_ms(events: &[&Event]) -> u64 {
    let mut attempt_intervals = BTreeMap::<&str, (u64, u64)>::new();
    for event in events {
        let Some(attempt_run_id) = event
            .metadata
            .get("agent_run_id")
            .filter(|run_id| !run_id.is_empty())
        else {
            continue;
        };
        attempt_intervals
            .entry(attempt_run_id)
            .and_modify(|(first, last)| {
                *first = (*first).min(event.timestamp_ms);
                *last = (*last).max(event.timestamp_ms);
            })
            .or_insert((event.timestamp_ms, event.timestamp_ms));
    }
    attempt_intervals
        .into_values()
        .fold(0_u64, |total, (first, last)| {
            total.saturating_add(last.saturating_sub(first))
        })
}

pub(crate) fn load_routing_telemetry_read_model(
    store: &mut SqliteStore,
) -> Result<Vec<RoutingTelemetry>, StorageError> {
    load_routing_telemetry_read_model_inner(store, true)
}

#[cfg(test)]
pub(crate) fn load_routing_telemetry_read_model_snapshot(
    store: &mut SqliteStore,
) -> Result<Vec<RoutingTelemetry>, StorageError> {
    load_routing_telemetry_read_model_inner(store, false)
}

fn load_routing_telemetry_read_model_inner(
    store: &mut SqliteStore,
    persist: bool,
) -> Result<Vec<RoutingTelemetry>, StorageError> {
    let task_id = phase16_task_id();
    let revision = store.event_revision(&task_id)?;
    let stored = store
        .load_read_model(
            ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
            ROUTING_TELEMETRY_READ_MODEL_KEY,
        )?
        .and_then(|stored| {
            serde_json::from_str::<RoutingTelemetryReadModel>(&stored.payload)
                .ok()
                .filter(|model| {
                    model.schema == ROUTING_TELEMETRY_READ_MODEL_NAMESPACE
                        && model.revision == stored.revision
                        && model.revision <= revision.latest_sequence
                        && model.event_count <= revision.event_count
                })
        });
    let rebuilt = stored.is_none();
    let mut model = stored.unwrap_or_else(|| RoutingTelemetryReadModel {
        schema: ROUTING_TELEMETRY_READ_MODEL_NAMESPACE.to_string(),
        revision: 0,
        event_count: 0,
        entries: Vec::new(),
    });
    let mut delta = store.list_by_task_after(&task_id, model.revision)?;
    let mut changed = rebuilt || !delta.is_empty();
    if model.event_count.saturating_add(delta.len() as u64) != revision.event_count {
        changed = true;
        model.revision = 0;
        model.event_count = 0;
        model.entries.clear();
        delta = store.list_by_task_after(&task_id, 0)?;
    }

    let terminal_events = delta
        .iter()
        .filter(|event| is_agent_run_terminal(event))
        .collect::<Vec<_>>();
    if rebuilt
        || terminal_events.iter().any(|event| {
            !event
                .metadata
                .contains_key(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
        })
    {
        let loaded_events;
        let all_events = if rebuilt {
            delta.as_slice()
        } else {
            loaded_events = store.list_by_task(&task_id)?;
            loaded_events.as_slice()
        };
        model.entries = routing_telemetry_entries_from_events(all_events)
            .into_iter()
            .map(|(run_id, telemetry)| RoutingTelemetryEntry { run_id, telemetry })
            .collect();
    } else {
        let completed_run_ids = terminal_events
            .iter()
            .filter_map(|event| event.metadata.get(LOGICAL_AGENT_RUN_ID_METADATA_KEY))
            .filter(|run_id| !run_id.is_empty())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut bridged_entries = BTreeMap::new();
        for run_id in completed_run_ids {
            let scopes = terminal_events
                .iter()
                .filter(|event| {
                    event.metadata.get(LOGICAL_AGENT_RUN_ID_METADATA_KEY) == Some(&run_id)
                })
                .map(|event| {
                    (
                        event.metadata.get("project_id").cloned(),
                        event.metadata.get("session_id").cloned(),
                    )
                })
                .collect::<BTreeSet<_>>();
            if scopes.len() != 1 {
                continue;
            }
            let Some(scope) = scopes.into_iter().next() else {
                continue;
            };
            let in_scope = |event: &Event| {
                event.metadata.get("project_id") == scope.0.as_ref()
                    && event.metadata.get("session_id") == scope.1.as_ref()
            };
            let run_events = store
                .list_by_task_and_metadata(&task_id, LOGICAL_AGENT_RUN_ID_METADATA_KEY, &run_id)?
                .into_iter()
                .filter(&in_scope)
                .collect::<Vec<_>>();
            let telemetry = if indexed_run_requires_lineage_bridge(&run_events) {
                if !bridged_entries.contains_key(&scope) {
                    let scope_events = match (scope.1.as_deref(), scope.0.as_deref()) {
                        (Some(session_id), _) => {
                            store.list_by_task_and_metadata(&task_id, "session_id", session_id)?
                        }
                        (None, Some(project_id)) => {
                            store.list_by_task_and_metadata(&task_id, "project_id", project_id)?
                        }
                        (None, None) => store.list_by_task(&task_id)?,
                    };
                    let events = scope_events
                        .into_iter()
                        .filter(|event| in_scope(event))
                        .collect::<Vec<_>>();
                    let entries = AgentRunLineage::from_events(&events)
                        .ok()
                        .map(|_| {
                            routing_telemetry_entries_from_events(&events)
                                .into_iter()
                                .collect::<BTreeMap<_, _>>()
                        })
                        .unwrap_or_default();
                    bridged_entries.insert(scope.clone(), entries);
                }
                bridged_entries
                    .get(&scope)
                    .and_then(|entries| entries.get(&run_id))
                    .cloned()
            } else {
                AgentRunLineage::from_events(&run_events)
                    .ok()
                    .and_then(|_| {
                        routing_telemetry_entries_from_events(&run_events)
                            .into_iter()
                            .find(|(logical_run_id, _)| logical_run_id == &run_id)
                            .map(|(_, telemetry)| telemetry)
                    })
            };
            let Some(telemetry) = telemetry else {
                continue;
            };
            model.entries.retain(|entry| entry.run_id != run_id);
            model
                .entries
                .push(RoutingTelemetryEntry { run_id, telemetry });
        }
    }
    if model.entries.len() > ROUTING_TELEMETRY_MAX_RUNS {
        model
            .entries
            .drain(0..model.entries.len() - ROUTING_TELEMETRY_MAX_RUNS);
    }
    model.revision = revision.latest_sequence;
    model.event_count = revision.event_count;
    if persist && changed {
        let payload = serde_json::to_string(&model).map_err(|error| {
            StorageError::new(format!("routing telemetry serialization failed: {error}"))
        })?;
        store.save_read_model(
            ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
            ROUTING_TELEMETRY_READ_MODEL_KEY,
            model.revision,
            &payload,
        )?;
    }
    Ok(model
        .entries
        .into_iter()
        .map(|entry| entry.telemetry)
        .collect())
}

#[cfg(test)]
pub(crate) fn routing_telemetry_from_events(events: &[Event]) -> Vec<RoutingTelemetry> {
    routing_telemetry_entries_from_events(events)
        .into_iter()
        .map(|(_, telemetry)| telemetry)
        .collect()
}

fn routing_telemetry_entries_from_events(events: &[Event]) -> Vec<(String, RoutingTelemetry)> {
    grouped_agent_run_events(events)
        .into_iter()
        .filter_map(|(logical_run_id, mut run_events)| {
            run_events.sort_by_key(|event| event.sequence);
            let started = run_events.iter().find(|event| {
                AgentRunEvent::from_event(event).is_some_and(AgentRunEvent::is_start)
            })?;
            let terminal = run_events
                .iter()
                .rev()
                .find(|event| is_agent_run_terminal(event))?;
            let event_epoch = |event: &Event| {
                event
                    .metadata
                    .get("steer_epoch")
                    .cloned()
                    .unwrap_or_else(|| "0".to_string())
            };
            let stable_epoch = event_epoch(terminal);
            let decision = run_events
                .iter()
                .rev()
                .find(|event| {
                    event.summary == "Agent run decision selected"
                        && event_epoch(event) == stable_epoch
                })
                .or_else(|| {
                    run_events.iter().rev().find(|event| {
                        AgentRunEvent::from_event(event).is_some_and(AgentRunEvent::is_start)
                            && event_epoch(event) == stable_epoch
                    })
                })
                .copied()?;
            let stable_events = run_events
                .iter()
                .copied()
                .filter(|event| event_epoch(event) == stable_epoch)
                .collect::<Vec<_>>();
            let task_class = parse_task_class_label(decision.metadata.get("task_class")?)?;
            let selected_policy = parse_policy(decision.metadata.get("collaboration_policy")?)?;
            let selected_model = decision
                .metadata
                .get("agent_model")
                .or_else(|| decision.metadata.get("router_model"))?
                .clone();
            let outcome = routing_outcome_for_run(&stable_events, terminal)?;
            let (quality_score, verification_passed) =
                routing_quality_signals(&stable_events, terminal);
            let learning_evidence = routing_learning_evidence(
                &stable_events,
                decision,
                terminal,
                stable_epoch.parse::<u64>().ok(),
            );
            let logical_cost_proxy = stable_events
                .iter()
                .filter(|event| event.kind == EventKind::ModelRequestFinished)
                .filter_map(|event| event.metadata.get("total_tokens"))
                .filter_map(|value| value.parse::<u64>().ok())
                .sum();
            let cost_proxy = learning_evidence
                .is_learnable()
                .then(|| learning_lineage_usage_from_metadata(&terminal.metadata))
                .flatten()
                .filter(|usage| usage.completeness == learning_evidence.usage_completeness)
                .map(|usage| usage.total_tokens)
                .unwrap_or(logical_cost_proxy);
            let tool_count = stable_events
                .iter()
                .filter(|event| event.kind == EventKind::ToolCallFinished)
                .count() as u64;
            let retrieval_count = stable_events
                .iter()
                .filter(|event| event.kind == EventKind::RetrievalPerformed)
                .count() as u64;
            Some((
                logical_run_id,
                RoutingTelemetry {
                    task_class,
                    context_signature: decision
                        .metadata
                        .get("routing_signature")
                        .cloned()
                        .unwrap_or_default(),
                    selected_policy,
                    selected_model,
                    latency_ms: active_agent_run_latency_ms(&stable_events),
                    outcome,
                    quality_score,
                    verification_passed,
                    learning_evidence,
                    cost_proxy,
                    tool_count,
                    retrieval_count,
                    user_override: started
                        .metadata
                        .get("requested_policy")
                        .map(|policy| policy != "auto_router")
                        .unwrap_or(false),
                },
            ))
        })
        .collect()
}
