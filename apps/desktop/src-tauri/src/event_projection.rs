use super::*;

pub(crate) fn request_mock_permission_in_store(
    store: &mut SqliteStore,
) -> Result<Phase3State, StorageError> {
    let task_id = phase3_task_id();
    let requested_at_ms = current_time_millis();
    let request_id = PermissionRequestId(unique_id("perm"));
    let mut metadata = Metadata::new();
    metadata.insert("command".to_string(), "echo phase-3".to_string());
    metadata.insert("cwd".to_string(), workspace_root().display().to_string());

    let request = PermissionRequest {
        id: request_id.clone(),
        task_id: task_id.clone(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "Run a harmless mock command to verify the permission gate.".to_string(),
        scope: workspace_root().display().to_string(),
        metadata,
    };

    append_event(
        store,
        &task_id,
        EventKind::ToolCallProposed,
        "Mock shell.run proposed",
        [
            ("tool".to_string(), request.action.clone()),
            ("permission_id".to_string(), request_id.0.clone()),
        ]
        .into_iter()
        .collect(),
    )?;

    store.save_permission_request(request.clone(), requested_at_ms)?;

    append_event(
        store,
        &task_id,
        EventKind::PermissionRequested,
        format!("Permission requested for {}", request.action),
        [
            ("permission_id".to_string(), request.id.0.clone()),
            (
                "risk".to_string(),
                permission_risk_label(&request.risk).to_string(),
            ),
            ("scope".to_string(), request.scope),
        ]
        .into_iter()
        .collect(),
    )?;

    phase3_state(store)
}

pub(crate) fn resolve_permission_in_store(
    store: &mut SqliteStore,
    request_id: &str,
    decision: &str,
) -> Result<Phase3State, StorageError> {
    let request_id = PermissionRequestId(request_id.to_string());
    let request = store
        .get_permission_request(&request_id)?
        .ok_or_else(|| StorageError::new("permission request not found"))?;
    let decision = parse_permission_decision(decision)?;
    let resolved_at_ms = current_time_millis();

    store.resolve_permission(PermissionResolution {
        request_id: request_id.clone(),
        decision: decision.clone(),
        resolved_at_ms,
        resolved_by: "local-user".to_string(),
    })?;

    append_event(
        store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(&decision)),
        [
            ("permission_id".to_string(), request_id.0),
            (
                "decision".to_string(),
                permission_decision_label(&decision).to_string(),
            ),
            ("tool".to_string(), request.action),
        ]
        .into_iter()
        .collect(),
    )?;

    phase3_state(store)
}

pub(crate) fn phase3_state(store: &SqliteStore) -> Result<Phase3State, StorageError> {
    let task_id = phase3_task_id();
    let audits = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id)
        .collect::<Vec<_>>();
    let timeline = store
        .list_by_task(&task_id)?
        .into_iter()
        .map(|event| timeline_entry(event, &audits))
        .collect();
    let permissions = audits.into_iter().map(permission_audit).collect();

    Ok(Phase3State {
        timeline,
        permissions,
    })
}

pub(crate) fn permission_review_state(
    store: &SqliteStore,
    project_sessions: &ProjectSessionConfig,
) -> Result<PermissionReviewState, StorageError> {
    let pending = store
        .list_permission_audits()?
        .into_iter()
        .filter(|record| record.resolution.is_none())
        .map(|record| permission_review_item(record, project_sessions))
        .collect();
    Ok(PermissionReviewState { pending })
}

pub(crate) fn permission_review_item(
    record: PermissionAuditRecord,
    project_sessions: &ProjectSessionConfig,
) -> PermissionReviewItem {
    let phase = record.request.metadata.get("phase").map(String::as_str);
    let source = match phase {
        Some("16") => "agent",
        Some("8") => "browser",
        Some("5") => "tool",
        _ if record.request.task_id == phase16_task_id() => "agent",
        _ if record.request.task_id == phase8_task_id() => "browser",
        _ if record.request.task_id == phase5_task_id() => "tool",
        _ => "test",
    };
    let mut session_id = record.request.metadata.get("session_id").cloned();
    if session_id.is_none() && source != "test" {
        session_id = project_sessions
            .active_session()
            .map(|session| session.id.clone());
    }
    let session = session_id.as_deref().and_then(|session_id| {
        project_sessions
            .sessions
            .iter()
            .find(|session| session.id == session_id)
    });
    let project_id = record
        .request
        .metadata
        .get("project_id")
        .cloned()
        .or_else(|| session.map(|session| session.project_id.clone()));
    let project = project_id.as_deref().and_then(|project_id| {
        project_sessions
            .projects
            .iter()
            .find(|project| project.id == project_id)
    });
    let session_name = record
        .request
        .metadata
        .get("session_name")
        .cloned()
        .or_else(|| session.map(|session| session.name.clone()));
    let project_name = record
        .request
        .metadata
        .get("project_name")
        .cloned()
        .or_else(|| project.map(|project| project.name.clone()));
    let input = ["tool_input", "command", "path"]
        .into_iter()
        .find_map(|key| record.request.metadata.get(key))
        .map(|value| redact_sensitive_text(value))
        .unwrap_or_default();
    let can_allow_session = source == "agent" && permission_can_allow_session(&record.request);
    // Same origin marker the thread's approval card uses: a write subagent's
    // request parks its parent run, so the review must stay actionable while
    // the session is busy.
    let subagent = record
        .request
        .metadata
        .get(crate::agent_subagent_runtime::SUBAGENT_PERMISSION_ORIGIN_KEY)
        .map(String::as_str)
        == Some(crate::agent_subagent_runtime::SUBAGENT_PERMISSION_ORIGIN_VALUE);

    PermissionReviewItem {
        request_id: record.request.id.0,
        action: redact_sensitive_text(&record.request.action),
        risk: permission_risk_label(&record.request.risk).to_string(),
        reason: redact_sensitive_text(&record.request.reason),
        scope: redact_sensitive_text(&record.request.scope),
        source: source.to_string(),
        project_id,
        project_name,
        session_id,
        session_name,
        input,
        requested_at_ms: record.requested_at_ms,
        can_allow_session,
        subagent,
    }
}

pub(crate) fn phase4_state(
    store: &mut SqliteStore,
    config: &ProviderConfig,
    last_error: Option<String>,
) -> Result<Phase4State, StorageError> {
    let task_id = phase4_task_id();
    let events = store.list_by_task(&task_id)?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let messages = events
        .iter()
        .filter_map(message_view_from_event)
        .collect::<Vec<_>>();

    Ok(Phase4State {
        provider: provider_config_state(config),
        timeline,
        messages,
        last_error,
    })
}

pub(crate) fn phase5_state(
    store: &SqliteStore,
    last_error: Option<String>,
    workspace_root: &Path,
) -> Result<Phase5State, StorageError> {
    let task_id = phase5_task_id();
    let events = store.list_by_task(&task_id)?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let results = events.iter().filter_map(tool_run_from_event).collect();
    let pending_approvals = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id && audit.resolution.is_none())
        .filter_map(tool_approval_from_audit)
        .collect();
    let tools = ToolRegistry::with_workspace_tools(workspace_root.to_path_buf())
        .specs()
        .into_iter()
        .map(|spec| ToolSpecView {
            name: spec.name,
            description: spec.description,
            risk: tool_risk_label(&spec.risk).to_string(),
            input_schema: spec.input_schema_json,
        })
        .collect();

    Ok(Phase5State {
        timeline,
        tools,
        pending_approvals,
        results,
        last_error,
    })
}

pub(crate) fn graph_count_summary_for_index_events(
    events: &[Event],
    active_index_path: &Path,
) -> GraphStateView {
    let active_index_path = active_index_path.display().to_string();
    events
        .iter()
        .rev()
        .find(|event| {
            event.kind == EventKind::RetrievalPerformed
                && event.metadata.get("action").map(String::as_str) == Some("index")
                && event.metadata.get("index_path") == Some(&active_index_path)
        })
        .map(|event| GraphStateView {
            total_nodes: event
                .metadata
                .get("graph_nodes")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            total_edges: event
                .metadata
                .get("graph_edges")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            nodes: Vec::new(),
            edges: Vec::new(),
        })
        .unwrap_or_else(empty_graph_state)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn phase7_state(
    store: &SqliteStore,
    stats: &RagIndexStats,
    memory: MemoryStatsView,
    sources: Vec<RagSourceView>,
    retrieval_trace: Option<RetrievalTraceView>,
    graph: GraphStateView,
    graph_summary_index_path: Option<&Path>,
    answer: Option<String>,
    last_error: Option<String>,
) -> Result<Phase7State, StorageError> {
    let events = store.list_by_task(&phase7_task_id())?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let answer = answer.or_else(|| events.iter().rev().find_map(rag_answer_from_event));
    let graph = graph_summary_index_path
        .map(|path| graph_count_summary_for_index_events(&events, path))
        .unwrap_or(graph);

    Ok(Phase7State {
        timeline,
        stats: rag_stats_view(stats),
        memory,
        sources,
        retrieval_trace,
        graph,
        answer,
        last_error,
    })
}

pub(crate) fn phase8_state(
    store: &SqliteStore,
    last_error: Option<String>,
) -> Result<Phase8State, StorageError> {
    let task_id = phase8_task_id();
    let events = store.list_by_task(&task_id)?;
    let timeline = events
        .iter()
        .cloned()
        .map(|event| timeline_entry(event, &[]))
        .collect();
    let observations = events
        .iter()
        .filter_map(browser_observation_from_event)
        .collect();
    let pending_approvals = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id && audit.resolution.is_none())
        .filter_map(tool_approval_from_audit)
        .collect();

    Ok(Phase8State {
        timeline,
        pending_approvals,
        observations,
        last_error,
    })
}

pub(crate) fn context_state(
    store: &SqliteStore,
    workspace_root: &Path,
    run_context: &Metadata,
    pack: Option<RestoreContextPack>,
    last_error: Option<String>,
) -> Result<ContextState, String> {
    let audits = store
        .list_permission_audits()
        .map_err(|error| error.to_string())?;
    let timeline = store
        .list_by_task(&phase15_task_id())
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|event| event_matches_context(event, run_context))
        .map(|event| timeline_entry(event, &audits))
        .collect();
    let checkpoint = match pack {
        Some(pack) => Some(context_checkpoint_view_from_pack(
            pack,
            Some(context_checkpoint_path_for_session(
                workspace_root,
                run_context.get("session_id").map(String::as_str),
            )),
        )),
        None => Some(live_context_checkpoint_view(
            store,
            workspace_root,
            run_context,
        )?),
    };

    Ok(ContextState {
        timeline,
        checkpoint,
        last_error,
    })
}

pub(crate) fn live_context_checkpoint_view(
    store: &SqliteStore,
    workspace_root: &Path,
    run_context: &Metadata,
) -> Result<ContextCheckpointView, String> {
    let events = collect_context_events(store, run_context).map_err(|error| error.to_string())?;
    let checkpoint =
        build_session_checkpoint_at(&events, CheckpointOptions::default(), current_time_millis());
    let mut pack = build_restore_context_pack(checkpoint);
    let messages = events
        .iter()
        .filter_map(message_from_event)
        .collect::<Vec<_>>();
    pack.text.push_str(&conversation_memory_to_markdown(
        &messages,
        CONTEXT_MEMORY_MAX_ITEMS,
    ));
    let checkpoint_path = context_checkpoint_path_for_session(
        workspace_root,
        run_context.get("session_id").map(String::as_str),
    );
    let path = if checkpoint_path.exists() {
        if let Ok(text) = fs::read_to_string(&checkpoint_path) {
            if !text.trim().is_empty() {
                pack.text = text;
            }
        }
        Some(checkpoint_path)
    } else {
        None
    };

    Ok(context_checkpoint_view_from_pack(pack, path))
}

pub(crate) fn context_checkpoint_view_from_pack(
    pack: RestoreContextPack,
    path: Option<PathBuf>,
) -> ContextCheckpointView {
    let SessionCheckpoint {
        id,
        generated_at_ms,
        event_count,
        task_count,
        latest_event_ms,
        current_goal,
        completed_steps,
        pending_steps,
        decisions,
        file_changes,
        commands_run,
        tool_results,
        retrievals,
        artifacts,
        errors,
        next_actions,
    } = pack.checkpoint;

    ContextCheckpointView {
        id,
        generated_at_ms,
        event_count,
        task_count,
        latest_event_ms,
        current_goal: current_goal.map(|value| redact_sensitive_text(&value)),
        completed_steps: redact_string_list(completed_steps),
        pending_steps: redact_string_list(pending_steps),
        decisions: redact_string_list(decisions),
        file_changes: redact_string_list(file_changes),
        commands_run: redact_string_list(commands_run),
        tool_results: redact_string_list(tool_results),
        retrievals: redact_string_list(retrievals),
        artifacts: redact_string_list(artifacts),
        errors: redact_string_list(errors),
        next_actions: redact_string_list(next_actions),
        path: path.map(|path| path.display().to_string()),
        restore_pack: redact_sensitive_text(&pack.text),
    }
}

pub(crate) fn redact_string_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| redact_sensitive_text(&value))
        .collect()
}

pub(crate) fn collect_context_events(
    store: &SqliteStore,
    run_context: &Metadata,
) -> Result<Vec<Event>, StorageError> {
    let mut events = Vec::new();
    for task_id in context_task_ids() {
        let task_events = if let Some(session_id) = run_context.get("session_id") {
            store.list_by_task_and_metadata(&task_id, "session_id", session_id)?
        } else if let Some(project_id) = run_context.get("project_id") {
            store.list_by_task_and_metadata(&task_id, "project_id", project_id)?
        } else {
            store.list_by_task(&task_id)?
        };
        events.extend(
            task_events
                .into_iter()
                .filter(|event| event_matches_context(event, run_context))
                .map(redact_event),
        );
    }
    events.sort_by(|left, right| {
        left.timestamp_ms
            .cmp(&right.timestamp_ms)
            .then(left.sequence.cmp(&right.sequence))
            .then(left.task_id.0.cmp(&right.task_id.0))
            .then(left.id.0.cmp(&right.id.0))
    });
    Ok(events)
}

pub(crate) fn event_matches_context(event: &Event, run_context: &Metadata) -> bool {
    if let Some(session_id) = run_context.get("session_id") {
        return event.metadata.get("session_id") == Some(session_id);
    }
    if let Some(project_id) = run_context.get("project_id") {
        return event.metadata.get("project_id") == Some(project_id);
    }
    true
}

pub(crate) fn write_context_checkpoint(
    workspace_root: &Path,
    session_id: Option<&str>,
    text: &str,
    coverage: Option<ContextCheckpointCoverage<'_>>,
) -> Result<PathBuf, String> {
    let path = context_checkpoint_path_for_session(workspace_root, session_id);
    let redacted_text = redact_sensitive_text(text);
    write_private_file_atomically(&path, redacted_text.as_bytes(), "context checkpoint")?;

    let manifest_path = context_checkpoint_manifest_path_for_session(workspace_root, session_id);
    if let Some(coverage) = coverage {
        let manifest = ContextCheckpointManifest::new(
            session_id,
            coverage.history,
            coverage.covered_messages,
            &redacted_text,
        )?;
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| format!("failed to encode context checkpoint manifest: {error}"))?;
        write_private_file_atomically(
            &manifest_path,
            &manifest_json,
            "context checkpoint manifest",
        )?;
    } else if let Err(error) = fs::remove_file(&manifest_path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!(
                "failed to remove stale context checkpoint manifest: {error}"
            ));
        }
    }

    Ok(path)
}

pub(crate) fn write_private_file_atomically(
    path: &Path,
    bytes: &[u8],
    label: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} path has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {label} directory: {error}"))?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("checkpoint");
    let temporary_path = parent.join(format!(".{file_name}.{}.tmp", unique_id("atomic-write")));

    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary_path)
            .map_err(|error| format!("failed to open temporary {label}: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("failed to write temporary {label}: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed to sync temporary {label}: {error}"))?;
        #[cfg(unix)]
        fs::set_permissions(&temporary_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("failed to secure temporary {label}: {error}"))?;
        fs::rename(&temporary_path, path)
            .map_err(|error| format!("failed to replace {label}: {error}"))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

pub(crate) fn write_agent_trace_jsonl(
    workspace_root: &Path,
    store: &SqliteStore,
    session_id: Option<&str>,
) -> Result<PathBuf, String> {
    let trace = agent_trace_state_for_session(store, None, None, session_id)
        .map_err(|error| error.to_string())?;
    let path = agent_trace_export_path_for(workspace_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create trace export directory: {error}"))?;
    }

    let mut lines = Vec::new();
    for turn in &trace.turns {
        for step in &turn.steps {
            lines.push(agent_trace_step_json(&trace, turn, step));
        }
    }
    let text = if lines.is_empty() {
        agent_trace_empty_json(&trace)
    } else {
        format!("{}\n", lines.join("\n"))
    };

    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|error| format!("failed to open trace export: {error}"))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("failed to write trace export: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure trace export: {error}"))?;

    Ok(path)
}

pub(crate) fn agent_trace_step_json(
    trace: &AgentTraceState,
    turn: &AgentTraceTurnView,
    step: &AgentTraceStepView,
) -> String {
    format!(
        "{{\"trace_id\":\"{}\",\"run_id\":\"{}\",\"task_id\":\"{}\",\"turn_index\":{},\"turn_status\":\"{}\",\"step_id\":\"{}\",\"parent_id\":{},\"sequence\":{},\"kind\":\"{}\",\"label\":\"{}\",\"status\":\"{}\",\"started_at_ms\":{},\"finished_at_ms\":{},\"latency_ms\":{},\"model\":{},\"tool_name\":{},\"request_id\":{},\"tool_call_id\":{},\"permission_id\":{},\"input_preview\":{},\"output_preview\":{},\"artifact_path\":{},\"detail\":\"{}\",\"metadata\":{}}}",
        trace_json_escape(&trace.trace_id),
        trace_json_escape(&trace.run_id),
        trace_json_escape(&trace.task_id),
        turn.index,
        trace_json_escape(&turn.status),
        trace_json_escape(&step.id),
        trace_optional_string_json(step.parent_id.as_deref()),
        step.sequence,
        trace_json_escape(&step.kind),
        trace_json_escape(&step.label),
        trace_json_escape(&step.status),
        step.started_at_ms,
        trace_optional_u64_json(step.finished_at_ms),
        trace_optional_u64_json(step.latency_ms),
        trace_optional_string_json(step.model.as_deref()),
        trace_optional_string_json(step.tool_name.as_deref()),
        trace_optional_string_json(step.request_id.as_deref()),
        trace_optional_string_json(step.tool_call_id.as_deref()),
        trace_optional_string_json(step.permission_id.as_deref()),
        trace_optional_string_json(step.input_preview.as_deref()),
        trace_optional_string_json(step.output_preview.as_deref()),
        trace_optional_string_json(step.artifact_path.as_deref()),
        trace_json_escape(&step.detail),
        trace_metadata_json(&step.metadata)
    )
}

pub(crate) fn agent_trace_empty_json(trace: &AgentTraceState) -> String {
    format!(
        "{{\"trace_id\":\"{}\",\"run_id\":\"{}\",\"task_id\":\"{}\",\"kind\":\"empty\",\"status\":\"{}\",\"step_count\":0}}\n",
        trace_json_escape(&trace.trace_id),
        trace_json_escape(&trace.run_id),
        trace_json_escape(&trace.task_id),
        trace_json_escape(&trace.status)
    )
}

pub(crate) fn trace_metadata_json(metadata: &Metadata) -> String {
    format!(
        "{{{}}}",
        metadata
            .iter()
            .map(|(key, value)| format!(
                "\"{}\":\"{}\"",
                trace_json_escape(key),
                trace_json_escape(value)
            ))
            .collect::<Vec<_>>()
            .join(",")
    )
}

pub(crate) fn trace_optional_string_json(value: Option<&str>) -> String {
    value
        .map(|value| format!("\"{}\"", trace_json_escape(value)))
        .unwrap_or_else(|| "null".to_string())
}

pub(crate) fn trace_optional_u64_json(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

pub(crate) fn trace_json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other if other.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}
