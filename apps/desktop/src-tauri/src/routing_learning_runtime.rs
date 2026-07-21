use super::*;

pub(crate) fn load_routing_telemetry_read_model(
    store: &mut SqliteStore,
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
    let mut model = stored.unwrap_or_else(|| RoutingTelemetryReadModel {
        schema: ROUTING_TELEMETRY_READ_MODEL_NAMESPACE.to_string(),
        revision: 0,
        event_count: 0,
        entries: Vec::new(),
    });
    let mut delta = store.list_by_task_after(&task_id, model.revision)?;
    if model.event_count.saturating_add(delta.len() as u64) != revision.event_count {
        model.revision = 0;
        model.event_count = 0;
        model.entries.clear();
        delta = store.list_by_task_after(&task_id, 0)?;
    }

    let completed_run_ids = delta
        .iter()
        .filter(|event| {
            matches!(
                event.summary.as_str(),
                "Agent task completed" | "Agent task cancelled" | "Agent task failed"
            )
        })
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
    let payload = serde_json::to_string(&model).map_err(|error| {
        StorageError::new(format!("routing telemetry serialization failed: {error}"))
    })?;
    store.save_read_model(
        ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
        ROUTING_TELEMETRY_READ_MODEL_KEY,
        model.revision,
        &payload,
    )?;
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
                matches!(
                    event.summary.as_str(),
                    "Agent task started" | "Agent task retry started"
                )
            })?;
            let task_class = parse_task_class_label(started.metadata.get("task_class")?)?;
            let selected_policy = parse_policy(started.metadata.get("collaboration_policy")?)?;
            let selected_model = started
                .metadata
                .get("agent_model")
                .or_else(|| started.metadata.get("router_model"))?
                .clone();
            let terminal = run_events.iter().rev().find(|event| {
                matches!(
                    event.summary.as_str(),
                    "Agent task completed" | "Agent task cancelled" | "Agent task failed"
                )
            })?;
            let outcome = routing_outcome_for_run(&run_events, terminal)?;
            let cost_proxy = run_events
                .iter()
                .filter(|event| event.kind == EventKind::ModelRequestFinished)
                .filter_map(|event| event.metadata.get("total_tokens"))
                .filter_map(|value| value.parse::<u64>().ok())
                .sum();
            let tool_count = run_events
                .iter()
                .filter(|event| event.kind == EventKind::ToolCallFinished)
                .count() as u64;
            let retrieval_count = run_events
                .iter()
                .filter(|event| event.kind == EventKind::RetrievalPerformed)
                .count() as u64;
            Some(RoutingTelemetry {
                task_class,
                context_signature: started
                    .metadata
                    .get("routing_signature")
                    .cloned()
                    .unwrap_or_default(),
                selected_policy,
                selected_model,
                latency_ms: terminal.timestamp_ms.saturating_sub(started.timestamp_ms),
                outcome,
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

pub(crate) fn completion_learning_signal(
    runtime: &agent_runtime::AgentLoopState,
) -> (&'static str, bool) {
    if runtime.successful_mutations == 0 {
        ("non_mutating", true)
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
    match terminal.summary.as_str() {
        "Agent task cancelled" => return Some(RoutingOutcome::UserRejected),
        "Agent task failed" => return Some(RoutingOutcome::Failed),
        "Agent task completed" => {}
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
            let quality_score = workflow_events
                .iter()
                .rev()
                .find(|event| event.summary == "Collaboration quality gate evaluated")
                .and_then(|event| event.metadata.get("quality_score"))
                .and_then(|score| score.parse::<f32>().ok());
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
            Some(WorkflowExecutionTelemetry {
                task_class,
                plan,
                succeeded: terminal.summary == "Collaboration workflow completed",
                quality_score,
                latency_ms: terminal.timestamp_ms.saturating_sub(planned.timestamp_ms),
                total_tokens,
                tool_calls,
                fallback_used: terminal
                    .metadata
                    .get("fallback_used")
                    .is_some_and(|value| value == "true"),
            })
        })
        .collect()
}
