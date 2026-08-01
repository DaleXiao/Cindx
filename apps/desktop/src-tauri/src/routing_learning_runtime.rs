use super::*;
pub(crate) use crate::learning_evidence_runtime::learning_budget_fingerprint;
#[cfg(test)]
pub(crate) use crate::learning_evidence_runtime::LEARNING_BUDGET_KEYS;
use crate::learning_evidence_runtime::{
    learning_lineage_usage_from_metadata, routing_learning_evidence, workflow_learning_evidence,
};

fn is_agent_run_terminal(event: &Event) -> bool {
    AgentRunEvent::from_event(event).is_some_and(|event| event.status().is_terminal())
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

    let completed_run_ids = delta
        .iter()
        .filter(|event| is_agent_run_terminal(event))
        .filter_map(|event| event.metadata.get("agent_run_id"))
        .cloned()
        .collect::<BTreeSet<_>>();
    for run_id in completed_run_ids {
        let run_events = store.list_by_task_and_metadata(&task_id, "agent_run_id", &run_id)?;
        let Some(telemetry) = routing_telemetry_from_events(&run_events)
            .into_iter()
            .next()
        else {
            continue;
        };
        model.entries.retain(|entry| entry.run_id != run_id);
        model
            .entries
            .push(RoutingTelemetryEntry { run_id, telemetry });
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

pub(crate) fn routing_telemetry_from_events(events: &[Event]) -> Vec<RoutingTelemetry> {
    let mut runs = BTreeMap::<String, Vec<&Event>>::new();
    for event in events {
        if let Some(run_id) = event.metadata.get("agent_run_id") {
            runs.entry(run_id.clone()).or_default().push(event);
        }
    }
    runs.into_values()
        .filter_map(|mut run_events| {
            run_events.sort_by_key(|event| event.sequence);
            let started = run_events.iter().find(|event| {
                AgentRunEvent::from_event(event).is_some_and(AgentRunEvent::is_start)
            })?;
            let decision = run_events
                .iter()
                .rev()
                .find(|event| event.summary == "Agent run decision selected")
                .copied()
                .unwrap_or(started);
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
            let stable_epoch = event_epoch(decision);
            if event_epoch(terminal) != stable_epoch {
                return None;
            }
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
            Some(RoutingTelemetry {
                task_class,
                context_signature: decision
                    .metadata
                    .get("routing_signature")
                    .cloned()
                    .unwrap_or_default(),
                selected_policy,
                selected_model,
                latency_ms: terminal.timestamp_ms.saturating_sub(
                    stable_events
                        .first()
                        .map(|event| event.timestamp_ms)
                        .unwrap_or(started.timestamp_ms),
                ),
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
            })
        })
        .collect()
}

pub(crate) fn load_workflow_telemetry_read_model(
    store: &mut SqliteStore,
    allowed_models: &[String],
) -> Result<Vec<WorkflowExecutionTelemetry>, StorageError> {
    let task_id = phase16_task_id();
    let revision = store.event_revision(&task_id)?;
    let model_pool_signature = workflow_model_pool_signature(allowed_models);
    let stored = store
        .load_read_model(
            WORKFLOW_TELEMETRY_READ_MODEL_NAMESPACE,
            WORKFLOW_TELEMETRY_READ_MODEL_KEY,
        )?
        .and_then(|stored| {
            serde_json::from_str::<WorkflowTelemetryReadModel>(&stored.payload)
                .ok()
                .filter(|model| {
                    model.schema == WORKFLOW_TELEMETRY_READ_MODEL_NAMESPACE
                        && model.revision == stored.revision
                        && model.revision <= revision.latest_sequence
                        && model.event_count <= revision.event_count
                        && model.model_pool_signature == model_pool_signature
                })
        });
    let rebuilt = stored.is_none();
    let mut model = stored.unwrap_or_else(|| WorkflowTelemetryReadModel {
        schema: WORKFLOW_TELEMETRY_READ_MODEL_NAMESPACE.to_string(),
        revision: 0,
        event_count: 0,
        model_pool_signature: model_pool_signature.clone(),
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
    let completed_workflows = delta
        .iter()
        .filter(|event| {
            matches!(
                event.summary.as_str(),
                "Collaboration workflow completed" | "Collaboration workflow failed"
            )
        })
        .filter_map(|event| event.metadata.get("collaboration_id"))
        .cloned()
        .collect::<BTreeSet<_>>();
    for workflow_id in completed_workflows {
        let workflow_events =
            store.list_by_task_and_metadata(&task_id, "collaboration_id", &workflow_id)?;
        model
            .entries
            .retain(|entry| entry.workflow_id != workflow_id);
        if let Some(telemetry) =
            workflow_execution_telemetry_from_events(&workflow_events, allowed_models)
                .into_iter()
                .next()
        {
            model.entries.push(WorkflowTelemetryEntry {
                workflow_id,
                telemetry,
            });
        }
    }
    if model.entries.len() > WORKFLOW_TELEMETRY_MAX_RUNS {
        model
            .entries
            .drain(0..model.entries.len() - WORKFLOW_TELEMETRY_MAX_RUNS);
    }
    model.revision = revision.latest_sequence;
    model.event_count = revision.event_count;
    model.model_pool_signature = model_pool_signature;
    if changed {
        let payload = serde_json::to_string(&model).map_err(|error| {
            StorageError::new(format!("workflow telemetry serialization failed: {error}"))
        })?;
        store.save_read_model(
            WORKFLOW_TELEMETRY_READ_MODEL_NAMESPACE,
            WORKFLOW_TELEMETRY_READ_MODEL_KEY,
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

fn workflow_model_pool_signature(allowed_models: &[String]) -> String {
    let mut models = allowed_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort_unstable();
    models.dedup();
    models.join("\u{1f}")
}

fn routing_quality_signals(run_events: &[&Event], terminal: &Event) -> (Option<f32>, Option<bool>) {
    if let Some(delivery) = run_events
        .iter()
        .rev()
        .find(|event| event.summary == "Collaboration workflow completed")
        .filter(|event| event.metadata.contains_key("anytime_selected_candidate"))
    {
        let quality_score = delivery
            .metadata
            .get("anytime_selected_quality_bps")
            .and_then(|score| score.parse::<f32>().ok())
            .map(|score| (score / 10_000.0).clamp(0.0, 1.0));
        let verification_passed = delivery
            .metadata
            .get("anytime_selected_verified")
            .and_then(|verified| verified.parse::<bool>().ok());
        return (quality_score, verification_passed);
    }

    if let Some(gate) = run_events
        .iter()
        .rev()
        .find(|event| event.summary == "Collaboration quality gate evaluated")
    {
        let quality_score = gate
            .metadata
            .get("quality_score")
            .and_then(|score| score.parse::<f32>().ok())
            .map(|score| score.clamp(0.0, 1.0));
        let quality_pass = gate
            .metadata
            .get("quality_pass")
            .and_then(|pass| pass.parse::<bool>().ok());
        let safety_clear = gate
            .metadata
            .get("safety_violations")
            .and_then(|count| count.parse::<usize>().ok())
            .map(|count| count == 0);
        return (
            quality_score,
            quality_pass
                .zip(safety_clear)
                .map(|(pass, clear)| pass && clear),
        );
    }

    let verification_passed = match terminal
        .metadata
        .get("completion_evidence")
        .map(String::as_str)
    {
        Some("verified_mutation") => Some(true),
        Some("unverified_mutation") => Some(false),
        _ => {
            let requested = terminal
                .metadata
                .get("verification_gate_requests")
                .and_then(|count| count.parse::<usize>().ok())
                .unwrap_or(0);
            let pending = terminal
                .metadata
                .get("pending_interaction_verifications")
                .and_then(|count| count.parse::<usize>().ok())
                .unwrap_or(0);
            (requested > 0).then_some(pending == 0)
        }
    };
    (None, verification_passed)
}

pub(crate) fn completion_learning_signal(
    runtime: &agent_runtime::AgentLoopState,
) -> (&'static str, bool) {
    if runtime.successful_mutations == 0 {
        ("non_mutating", false)
    } else if runtime.verified_after_last_mutation {
        ("verified_mutation", true)
    } else {
        ("unverified_mutation", false)
    }
}

pub(crate) fn routing_outcome_for_run(
    run_events: &[&Event],
    terminal: &Event,
) -> Option<RoutingOutcome> {
    match AgentRunEvent::from_event(terminal).map(AgentRunEvent::status) {
        Some(AgentRunStatus::Cancelled) => return Some(RoutingOutcome::UserRejected),
        Some(AgentRunStatus::Failed) => return Some(RoutingOutcome::Failed),
        Some(AgentRunStatus::Completed) => {}
        _ => return None,
    }

    match terminal.metadata.get("routing_learning_eligible") {
        Some(value) if value == "false" => return None,
        Some(_) => {}
        None => {
            let has_legacy_successful_tool = run_events.iter().any(|event| {
                event.kind == EventKind::ToolCallFinished
                    && event.metadata.get("status").map(String::as_str) == Some("succeeded")
            });
            if has_legacy_successful_tool {
                return None;
            }
        }
    }

    if let Some(delivery) = run_events
        .iter()
        .rev()
        .find(|event| event.summary == "Collaboration workflow completed")
        .filter(|event| event.metadata.contains_key("anytime_selected_candidate"))
    {
        if delivery
            .metadata
            .get("anytime_routing_learning_eligible")
            .is_some_and(|eligible| eligible == "false")
        {
            return None;
        }
        let verified = delivery
            .metadata
            .get("anytime_selected_verified")?
            .parse::<bool>()
            .ok()?;
        let quality_bps = delivery
            .metadata
            .get("anytime_selected_quality_bps")?
            .parse::<u16>()
            .ok()?;
        return Some(
            if verified && quality_bps >= (ADAPTIVE_QUALITY_PASS_SCORE * 10_000.0).round() as u16 {
                RoutingOutcome::Succeeded
            } else {
                RoutingOutcome::Failed
            },
        );
    }

    if let Some(gate) = run_events
        .iter()
        .rev()
        .find(|event| event.summary == "Collaboration quality gate evaluated")
    {
        let pass = gate.metadata.get("quality_pass")?.parse::<bool>().ok()?;
        let score = gate.metadata.get("quality_score")?.parse::<f32>().ok()?;
        let safety_violations = gate
            .metadata
            .get("safety_violations")?
            .parse::<usize>()
            .ok()?;
        return Some(
            if pass && score >= ADAPTIVE_QUALITY_PASS_SCORE && safety_violations == 0 {
                RoutingOutcome::Succeeded
            } else {
                RoutingOutcome::Failed
            },
        );
    }

    if run_events
        .iter()
        .any(|event| event.summary == "Collaboration quality gate unavailable")
    {
        return None;
    }

    Some(RoutingOutcome::Succeeded)
}

pub(crate) fn workflow_execution_telemetry_from_events(
    events: &[Event],
    allowed_models: &[String],
) -> Vec<WorkflowExecutionTelemetry> {
    let mut workflows = BTreeMap::<String, Vec<&Event>>::new();
    for event in events {
        if let Some(workflow_id) = event.metadata.get("collaboration_id") {
            workflows
                .entry(workflow_id.clone())
                .or_default()
                .push(event);
        }
    }

    workflows
        .into_values()
        .filter_map(|mut workflow_events| {
            workflow_events.sort_by_key(|event| event.sequence);
            let planned = workflow_events.iter().find(|event| {
                event.summary == "Collaboration workflow planned"
                    && event.metadata.contains_key("workflow_ir")
            })?;
            let plan =
                WorkflowPlanIr::from_json(planned.metadata.get("workflow_ir")?, allowed_models)
                    .ok()?;
            let terminal = workflow_events.iter().rev().find(|event| {
                matches!(
                    event.summary.as_str(),
                    "Collaboration workflow completed" | "Collaboration workflow failed"
                )
            })?;
            let task_class = parse_task_class_label(planned.metadata.get("task_class")?)?;
            let quality_score = terminal
                .metadata
                .get("anytime_selected_quality_bps")
                .and_then(|score| score.parse::<f32>().ok())
                .map(|score| (score / 10_000.0).clamp(0.0, 1.0))
                .or_else(|| {
                    workflow_events
                        .iter()
                        .rev()
                        .find(|event| event.summary == "Collaboration quality gate evaluated")
                        .and_then(|event| event.metadata.get("quality_score"))
                        .and_then(|score| score.parse::<f32>().ok())
                });
            let total_tokens = workflow_events
                .iter()
                .filter(|event| event.kind == EventKind::ModelRequestFinished)
                .filter_map(|event| event.metadata.get("total_tokens"))
                .filter_map(|tokens| tokens.parse::<u64>().ok())
                .sum();
            let tool_calls = workflow_events
                .iter()
                .filter(|event| event.kind == EventKind::ToolCallFinished)
                .count() as u64;
            let successful_tools_by_step = workflow_events
                .iter()
                .filter(|event| event.kind == EventKind::ToolCallFinished)
                .filter(|event| {
                    event
                        .metadata
                        .get("status")
                        .is_some_and(|status| matches!(status.as_str(), "succeeded" | "success"))
                })
                .filter_map(|event| {
                    let stage = event.metadata.get("stage")?;
                    let index = stage.strip_prefix("worker_")?.parse::<usize>().ok()?;
                    let step_id = plan.steps.get(index.checked_sub(1)?)?.id.clone();
                    let tool = event.metadata.get("tool")?.clone();
                    Some((step_id, tool))
                })
                .fold(
                    BTreeMap::<String, Vec<String>>::new(),
                    |mut tools, (step_id, tool)| {
                        tools.entry(step_id).or_default().push(tool);
                        tools
                    },
                );
            let learning_evidence = workflow_learning_evidence(&workflow_events, planned, terminal);
            let anchor_latency_ms = workflow_events
                .iter()
                .rev()
                .find(|event| {
                    event.kind == EventKind::ModelRequestFinished
                        && event.metadata.get("stage").map(String::as_str)
                            == Some("direct_anchor")
                })
                .and_then(|event| event.metadata.get("latency_ms"))
                .and_then(|latency| latency.parse::<u64>().ok());
            Some(WorkflowExecutionTelemetry {
                task_class,
                routing_signature: planned
                    .metadata
                    .get("routing_signature")
                    .cloned()
                    .unwrap_or_default(),
                plan,
                succeeded: terminal.summary == "Collaboration workflow completed",
                quality_score,
                learning_evidence,
                latency_ms: terminal.timestamp_ms.saturating_sub(planned.timestamp_ms),
                total_tokens,
                tool_calls,
                successful_tools_by_step,
                fallback_used: terminal
                    .metadata
                    .get("fallback_used")
                    .is_some_and(|value| value == "true"),
                paired_team_score_bps: terminal
                    .metadata
                    .get("anytime_team_score_bps")
                    .and_then(|score| score.parse::<u16>().ok()),
                paired_anchor_score_bps: terminal
                    .metadata
                    .get("anytime_anchor_score_bps")
                    .and_then(|score| score.parse::<u16>().ok()),
                paired_uplift_bps: terminal
                    .metadata
                    .get("anytime_team_uplift_bps")
                    .and_then(|uplift| uplift.parse::<i16>().ok()),
                selected_anchor: terminal
                    .metadata
                    .get("anytime_selected_kind")
                    .is_some_and(|kind| kind == "direct_anchor"),
                anchor_latency_ms,
            })
        })
        .collect()
}
