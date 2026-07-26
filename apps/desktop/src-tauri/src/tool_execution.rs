use super::*;

pub(crate) fn phase8_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<Phase8State, String> {
    let message = message.into();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase8_task_id(),
        EventKind::Error,
        "Browser tool failed",
        [("error".to_string(), message.clone())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase8_state(&store, Some(message)).map_err(|error| error.to_string())
}

pub(crate) fn phase6_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<Phase6State, String> {
    let message = message.into();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase6_task_id(),
        EventKind::Error,
        "Orchestration failed",
        [("error".to_string(), message.clone())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase6_state(&store, Some(message)).map_err(|error| error.to_string())
}

pub(crate) fn phase5_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<Phase5State, String> {
    let message = message.into();
    let root = active_workspace_root(state)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase5_task_id(),
        EventKind::Error,
        "Tool call failed",
        [("error".to_string(), message.clone())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase5_state(&store, Some(message), &root).map_err(|error| error.to_string())
}

pub(crate) fn execute_tool_invocation(
    store: &mut SqliteStore,
    invocation: ToolInvocation,
    workspace_root: &Path,
    registry: Option<&ToolRegistry>,
) -> Result<(), StorageError> {
    execute_tool_invocation_with_result(store, invocation, workspace_root, registry, None)
        .map(|_| ())
}

pub(crate) fn execute_agent_tool_invocation(
    state: &tauri::State<'_, AppState>,
    registry: &ToolRegistry,
    mut invocation: ToolInvocation,
    workspace_root: &Path,
    run_context: &Metadata,
) -> Result<ToolResult, String> {
    for (key, value) in run_context {
        invocation
            .metadata
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    let task_id = invocation.task_id.clone();
    let tool_call_id = invocation.id.0.clone();
    let tool_name = invocation.tool_name.clone();
    let tool_input = invocation.input_json.clone();
    let input_fingerprint = tool_input_fingerprint(&tool_name, &tool_input);
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        if let Some(result) = completed_tool_result(&store, &invocation, workspace_root)
            .map_err(|error| error.to_string())?
        {
            return Ok(result);
        }
        append_event(
            &mut store,
            &task_id,
            EventKind::ToolCallStarted,
            format!("Tool call started: {tool_name}"),
            metadata_with_context(tool_invocation_event_metadata(&invocation), run_context),
        )
        .map_err(|error| error.to_string())?;
    }

    let execution_started_at = Instant::now();
    let run_control =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
    let scope = run_context
        .get("stage")
        .or_else(|| run_context.get("collaboration_stage"))
        .map(String::as_str)
        .unwrap_or("executor");
    let budget_stop = run_control.as_ref().and_then(|control| {
        control
            .begin_tool_call(scope, &tool_name, &invocation.input_json)
            .err()
    });
    let tool_control = ToolExecutionControl::new({
        let run_control = run_control.clone();
        move || {
            run_control
                .as_ref()
                .is_some_and(|control| control.should_stop())
        }
    });
    let mutates_workspace = registry
        .get(&tool_name)
        .is_some_and(|tool| tool_may_mutate_workspace(&tool_name, &tool.spec().risk));
    let tool_started = budget_stop.is_none();
    let mut result = if let Some(reason) = budget_stop {
        ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Cancelled,
            format!("Tool execution stopped: {}.", reason.code()),
            [("stop_reason".to_string(), reason.code().to_string())]
                .into_iter()
                .collect(),
        )
    } else {
        match registry.get(&tool_name) {
            Some(tool) => match tool.execute_with_control(invocation, &tool_control) {
                Ok(result) => result,
                Err(error) => {
                    failed_tool_result(agent_core::ToolCallId(tool_call_id.clone()), error)
                }
            },
            None => {
                ToolResult::failed(agent_core::ToolCallId(tool_call_id.clone()), "unknown tool")
            }
        }
    };
    if let Some(control) = run_control.as_ref() {
        if tool_started {
            control.finish_tool_call();
        }
        control.mark_progress("tool_result", &tool_name);
        if matches!(result.status, ToolOutcomeStatus::Succeeded) {
            control.record_checkpoint(
                "tool_result",
                &tool_name,
                &format!("{tool_name}\n{tool_input}\n{}", result.output),
            );
        }
        let progress = control.progress();
        result.metadata.insert(
            "run_checkpoints".to_string(),
            progress.checkpoints.to_string(),
        );
        result.metadata.insert(
            "run_observations".to_string(),
            progress.observations.to_string(),
        );
        result.metadata.insert(
            "run_budget_extensions".to_string(),
            progress.budget_extensions.to_string(),
        );
    }
    materialize_tool_result_artifacts(&mut result, workspace_root)?;
    finalize_tool_result(
        &mut result,
        &agent_core::ToolCallId(tool_call_id.clone()),
        &input_fingerprint,
        execution_started_at.elapsed(),
    );
    if mutates_workspace && matches!(result.status, ToolOutcomeStatus::Succeeded) {
        if let Err(error) = invalidate_workspace_knowledge_cache(state, workspace_root) {
            eprintln!("workspace knowledge cache invalidation failed: {error}");
        }
    }

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_tool_finished_event(
        &mut store,
        &task_id,
        &tool_call_id,
        &tool_name,
        tool_outcome_label(&result.status),
        &result.output,
        result.metadata.clone(),
        Some(run_context),
    )
    .map_err(|error| error.to_string())?;
    Ok(result)
}

pub(crate) fn tool_may_mutate_workspace(tool_name: &str, risk: &ToolRisk) -> bool {
    if tool_name.starts_with("browser.") || tool_name.starts_with("computer.") {
        return false;
    }
    !matches!(risk, ToolRisk::ReadOnly)
}

pub(crate) fn observation_from_agent_tool_result(tool_name: &str, result: &ToolResult) -> String {
    let mut output = result.output.clone();
    if let Some(failure) = &result.failure {
        output = format!(
            "failure_code={}\nretryable={}\n{}",
            failure.code, failure.retryable, output
        );
    }
    if !result.artifacts.is_empty() {
        output.push_str("\n\nArtifacts available in the active workspace:\n");
        output.push_str(
            &result
                .artifacts
                .iter()
                .map(|artifact| format!("- {}", artifact.path))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    observation_from_tool_result(tool_name, tool_outcome_label(&result.status), &output)
}

pub(crate) fn tool_result_image_paths(result: &ToolResult) -> Vec<String> {
    result
        .artifacts
        .iter()
        .filter(|artifact| {
            artifact
                .mime_type
                .as_deref()
                .is_some_and(|mime_type| mime_type.starts_with("image/"))
                || matches!(
                    Path::new(&artifact.path)
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .as_str(),
                    "avif" | "gif" | "jpeg" | "jpg" | "png" | "webp"
                )
        })
        .map(|artifact| artifact.path.clone())
        .collect()
}

pub(crate) fn append_visual_reference_message(
    runtime: &mut agent_runtime::AgentLoopState,
    tool_name: &str,
    image_paths: &[String],
) {
    if image_paths.is_empty() {
        return;
    }
    runtime.messages.push(Message {
        role: MessageRole::User,
        content: format!(
            "Visual reference captured by {tool_name}. Inspect the attached screenshot before deciding the next action."
        ),
        metadata: [
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), "visual_reference".to_string()),
            ("tool".to_string(), tool_name.to_string()),
            ("image_paths".to_string(), image_paths.join("\n")),
        ]
        .into_iter()
        .collect(),
    });
}

pub(crate) fn append_visual_reference_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    tool_name: &str,
    image_paths: &[String],
    run_context: &Metadata,
) -> Result<(), String> {
    if image_paths.is_empty() {
        return Ok(());
    }
    append_message_event_with_metadata(
        store,
        task_id,
        MessageRole::User,
        &format!(
            "Visual reference captured by {tool_name}. Inspect the attached screenshot before deciding the next action."
        ),
        metadata_with_context(
            [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "visual_reference".to_string()),
                ("tool".to_string(), tool_name.to_string()),
                ("image_paths".to_string(), image_paths.join("\n")),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn materialize_tool_result_artifacts(
    result: &mut ToolResult,
    workspace_root: &Path,
) -> Result<(), String> {
    const MAX_INLINE_STRUCTURED_OUTPUT_BYTES: usize = 256 * 1024;
    let output_dir = workspace_root.join(".cindx").join("artifacts");
    let artifact_stem: String = result
        .invocation_id
        .0
        .chars()
        .take(96)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let mut image_index = 0usize;
    for content in &mut result.content {
        let ToolContent::Image { mime_type, data } = content else {
            continue;
        };
        fs::create_dir_all(&output_dir)
            .map_err(|error| format!("failed to create tool artifact directory: {error}"))?;
        let extension = match mime_type.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        let filename = format!("{artifact_stem}-{image_index}.{extension}");
        let path = output_dir.join(&filename);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|error| format!("invalid tool image data: {error}"))?;
        fs::write(&path, bytes)
            .map_err(|error| format!("failed to write tool image artifact: {error}"))?;
        result.artifacts.push(ToolArtifact {
            path: path.display().to_string(),
            mime_type: Some(mime_type.clone()),
            title: Some("MCP image output".to_string()),
        });
        data.clear();
        image_index += 1;
    }
    for key in ["artifact_path", "screenshot_path", "text_path"] {
        let Some(path) = result.metadata.get(key).cloned() else {
            continue;
        };
        let path = if Path::new(&path).is_absolute() {
            PathBuf::from(path)
        } else {
            workspace_root.join(path)
        };
        let path = path.display().to_string();
        if result.artifacts.iter().any(|artifact| {
            let artifact_path = Path::new(&artifact.path);
            let artifact_path = if artifact_path.is_absolute() {
                artifact_path.to_path_buf()
            } else {
                workspace_root.join(artifact_path)
            };
            artifact_path == Path::new(&path)
        }) {
            continue;
        }
        let mime_type = match Path::new(&path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "avif" => Some("image/avif"),
            "gif" => Some("image/gif"),
            "jpeg" | "jpg" => Some("image/jpeg"),
            "png" => Some("image/png"),
            "webp" => Some("image/webp"),
            "html" | "htm" => Some("text/html"),
            "md" | "markdown" => Some("text/markdown"),
            "txt" => Some("text/plain"),
            _ => None,
        }
        .map(str::to_string);
        result.artifacts.push(ToolArtifact {
            path,
            mime_type,
            title: None,
        });
    }
    if let Some(structured) = result.structured_output_json.take() {
        if structured.len() > MAX_INLINE_STRUCTURED_OUTPUT_BYTES {
            fs::create_dir_all(&output_dir)
                .map_err(|error| format!("failed to create tool artifact directory: {error}"))?;
            let path = output_dir.join(format!("{artifact_stem}-structured.json"));
            write_private_file_atomically(
                &path,
                structured.as_bytes(),
                "structured tool output artifact",
            )?;
            let reference = serde_json::json!({
                "schema": "cindx.tool-output-reference.v1",
                "artifact_path": path.display().to_string(),
                "bytes": structured.len(),
            })
            .to_string();
            result.artifacts.push(ToolArtifact {
                path: path.display().to_string(),
                mime_type: Some("application/json".to_string()),
                title: Some("Structured tool output".to_string()),
            });
            result.metadata.insert(
                "structured_output_path".to_string(),
                path.display().to_string(),
            );
            result.metadata.insert(
                "structured_output_bytes".to_string(),
                structured.len().to_string(),
            );
            result
                .metadata
                .insert("structured_output".to_string(), reference.clone());
            result.structured_output_json = Some(reference);
        } else {
            result
                .metadata
                .insert("structured_output".to_string(), structured.clone());
            result.structured_output_json = Some(structured);
        }
    }
    if !result.artifacts.is_empty() {
        result.metadata.insert(
            "artifact_count".to_string(),
            result.artifacts.len().to_string(),
        );
        let primary_artifact_path = result
            .artifacts
            .iter()
            .find(|artifact| {
                artifact
                    .mime_type
                    .as_deref()
                    .is_some_and(|mime_type| mime_type.starts_with("image/"))
            })
            .unwrap_or(&result.artifacts[0])
            .path
            .clone();
        result
            .metadata
            .insert("artifact_path".to_string(), primary_artifact_path.clone());
        preserve_primary_tool_artifact_version(
            result,
            workspace_root,
            &artifact_stem,
            &primary_artifact_path,
        )?;
    }
    Ok(())
}

fn preserve_primary_tool_artifact_version(
    result: &mut ToolResult,
    workspace_root: &Path,
    artifact_stem: &str,
    primary_artifact_path: &str,
) -> Result<(), String> {
    if !matches!(result.status, ToolOutcomeStatus::Succeeded) {
        return Ok(());
    }
    let canonical_root = fs::canonicalize(workspace_root)
        .map_err(|error| format!("failed to resolve tool artifact workspace: {error}"))?;
    let source = Path::new(primary_artifact_path);
    let source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        workspace_root.join(source)
    };
    let Ok(source) = fs::canonicalize(source) else {
        return Ok(());
    };
    if !source.is_file() || !source.starts_with(&canonical_root) {
        return Ok(());
    }
    let relative_source = source
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("failed to resolve tool artifact path: {error}"))?;
    let output_history_root = Path::new(".cindx").join("output-history");
    if relative_source.starts_with(&output_history_root) {
        return Ok(());
    }

    let snapshot_relative = output_history_root
        .join("tool-results")
        .join(artifact_stem)
        .join(relative_source);
    let snapshot = workspace_root.join(&snapshot_relative);
    if let Some(parent) = snapshot.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create output history directory: {error}"))?;
    }
    fs::copy(&source, &snapshot)
        .map_err(|error| format!("failed to preserve tool output version: {error}"))?;
    result.metadata.insert(
        "source_path".to_string(),
        relative_source.display().to_string(),
    );
    result.metadata.insert(
        "artifact_path".to_string(),
        snapshot_relative.display().to_string(),
    );
    Ok(())
}

pub(crate) fn execute_tool_invocation_with_result(
    store: &mut SqliteStore,
    invocation: ToolInvocation,
    workspace_root: &Path,
    registry: Option<&ToolRegistry>,
    run_context: Option<&Metadata>,
) -> Result<ToolResult, StorageError> {
    if let Some(result) = completed_tool_result(store, &invocation, workspace_root)? {
        return Ok(result);
    }
    let invocation_context = tool_invocation_context(&invocation);
    let event_context =
        run_context.or_else(|| (!invocation_context.is_empty()).then_some(&invocation_context));
    let metadata = tool_invocation_event_metadata(&invocation);
    let metadata = match run_context {
        Some(context) => metadata_with_context(metadata, context),
        None => metadata,
    };
    append_event(
        store,
        &invocation.task_id,
        EventKind::ToolCallStarted,
        format!("Tool call started: {}", invocation.tool_name),
        metadata,
    )?;

    let execution_started_at = Instant::now();
    let task_id = invocation.task_id.clone();
    let tool_call_id = invocation.id.0.clone();
    let tool_name = invocation.tool_name.clone();
    let input_fingerprint = tool_input_fingerprint(&tool_name, &invocation.input_json);
    let fallback_registry = registry
        .is_none()
        .then(|| ToolRegistry::with_workspace_tools(workspace_root.to_path_buf()));
    let registry = registry
        .or(fallback_registry.as_ref())
        .expect("tool registry should be available");
    let Some(tool) = registry.get(&invocation.tool_name) else {
        let mut result = ToolResult::failed(invocation.id, "unknown tool");
        finalize_tool_result(
            &mut result,
            &agent_core::ToolCallId(tool_call_id.clone()),
            &input_fingerprint,
            execution_started_at.elapsed(),
        );
        append_tool_finished_event(
            store,
            &task_id,
            &tool_call_id,
            &tool_name,
            "failed",
            "unknown tool",
            result.metadata.clone(),
            event_context,
        )?;
        return Ok(result);
    };

    let result = match tool.execute(invocation) {
        Ok(mut result) => {
            finalize_tool_result(
                &mut result,
                &agent_core::ToolCallId(tool_call_id.clone()),
                &input_fingerprint,
                execution_started_at.elapsed(),
            );
            append_tool_finished_event(
                store,
                &task_id,
                &tool_call_id,
                &tool_name,
                tool_outcome_label(&result.status),
                &result.output,
                result.metadata.clone(),
                event_context,
            )?;
            result
        }
        Err(error) => {
            let mut result =
                failed_tool_result(agent_core::ToolCallId(tool_call_id.clone()), error);
            finalize_tool_result(
                &mut result,
                &agent_core::ToolCallId(tool_call_id.clone()),
                &input_fingerprint,
                execution_started_at.elapsed(),
            );
            append_tool_finished_event(
                store,
                &task_id,
                &tool_call_id,
                &tool_name,
                "failed",
                &result.output,
                result.metadata.clone(),
                event_context,
            )?;
            result
        }
    };

    Ok(result)
}

pub(crate) fn append_tool_proposed_event(
    store: &mut SqliteStore,
    invocation: &ToolInvocation,
    run_context: Option<&Metadata>,
) -> Result<(), StorageError> {
    let mut metadata = tool_invocation_event_metadata(invocation);
    metadata.insert(
        "input_preview".to_string(),
        truncate_for_timeline(&invocation.input_json),
    );
    let metadata = match run_context {
        Some(context) => metadata_with_context(metadata, context),
        None => metadata,
    };
    append_event(
        store,
        &invocation.task_id,
        EventKind::ToolCallProposed,
        format!("Tool call proposed: {}", invocation.tool_name),
        metadata,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_tool_finished_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    tool_call_id: &str,
    tool_name: &str,
    status: &str,
    output: &str,
    result_metadata: Metadata,
    run_context: Option<&Metadata>,
) -> Result<(), StorageError> {
    let mut metadata = Metadata::new();
    metadata.insert("tool_call_id".to_string(), tool_call_id.to_string());
    metadata.insert("tool".to_string(), tool_name.to_string());
    metadata.insert("status".to_string(), status.to_string());
    metadata.insert("output".to_string(), output.to_string());
    metadata.insert("output_length".to_string(), output.len().to_string());
    for (key, value) in result_metadata {
        metadata.insert(format!("result_{key}"), value);
    }
    if let Some(context) = run_context {
        metadata = metadata_with_context(metadata, context);
    }
    compact_tool_event_metadata(&mut metadata);

    append_event(
        store,
        task_id,
        EventKind::ToolCallFinished,
        format!("Tool call finished: {tool_name}"),
        metadata,
    )
}

pub(crate) fn phase4_state_with_error(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    message: &str,
) -> Result<Phase4State, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase4_state(&mut store, config, Some(message.to_string())).map_err(|error| error.to_string())
}

pub(crate) fn record_phase4_error(
    state: &tauri::State<'_, AppState>,
    message: &str,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase4_task_id(),
        EventKind::Error,
        "Model request failed",
        [("error".to_string(), message.to_string())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn timeline_entry(event: Event, audits: &[PermissionAuditRecord]) -> TimelineEntry {
    let permission_id = event.metadata.get("permission_id");
    let permission_is_pending = permission_id.is_some_and(|id| {
        audits
            .iter()
            .any(|audit| audit.request.id.0 == *id && audit.resolution.is_none())
    });
    let workflow_progress = timeline_workflow_progress(&event);
    let detail = match event.kind {
        EventKind::MessageAdded => event
            .metadata
            .get("content")
            .map(|content| {
                format!(
                    "{}: {}",
                    event
                        .metadata
                        .get("role")
                        .cloned()
                        .unwrap_or_else(|| "message".to_string()),
                    truncate_for_timeline(content)
                )
            })
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::ModelRequestStarted => event
            .metadata
            .get("model")
            .map(|model| format!("{} using {model}", event.summary))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::ModelRequestFinished => {
            let latency = event
                .metadata
                .get("latency_ms")
                .cloned()
                .unwrap_or_default();
            if latency.is_empty() {
                event.summary.clone()
            } else {
                format!("{} in {latency} ms", event.summary)
            }
        }
        EventKind::ToolCallProposed => event
            .metadata
            .get("input_preview")
            .map(|input| format!("{} with {}", event.summary, input.replace('\n', " ")))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::ToolCallStarted => event
            .metadata
            .get("tool")
            .map(|tool| format!("Executing {tool}"))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::ToolCallFinished => {
            let status = event
                .metadata
                .get("status")
                .cloned()
                .unwrap_or_else(|| "done".to_string());
            let output = event
                .metadata
                .get("output")
                .map(|value| truncate_for_timeline(value))
                .unwrap_or_default();
            if output.is_empty() {
                format!("{}: {status}", event.summary)
            } else {
                format!("{}: {status}. {output}", event.summary)
            }
        }
        EventKind::PermissionRequested => event
            .metadata
            .get("scope")
            .map(|scope| format!("{} Scope: {scope}", event.summary))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::PermissionResolved => event
            .metadata
            .get("decision")
            .map(|decision| format!("{} with {decision}", event.summary))
            .unwrap_or_else(|| event.summary.clone()),
        EventKind::Error => event
            .metadata
            .get("error")
            .cloned()
            .unwrap_or_else(|| event.summary.clone()),
        _ => event.summary.clone(),
    };

    TimelineEntry {
        sequence: event.sequence,
        label: timeline_event_label(&event),
        detail: redact_sensitive_text(&detail),
        kind: event_kind_ui_kind(&event.kind).to_string(),
        state: event_state(&event.kind, permission_is_pending).to_string(),
        timestamp_ms: event.timestamp_ms,
        workflow_progress,
    }
}

pub(crate) fn timeline_workflow_progress(event: &Event) -> Option<WorkflowProgressView> {
    let total_steps = event
        .metadata
        .get("workflow_steps")?
        .parse::<usize>()
        .ok()?;
    if total_steps == 0 || !event.metadata.contains_key("workflow_checkpoint_schema") {
        return None;
    }
    let completed_steps = event
        .metadata
        .get("completed_steps")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default()
        .min(total_steps);
    let step_status = event.metadata.get("step_status").cloned();
    Some(WorkflowProgressView {
        completed_steps,
        total_steps,
        current_step_id: event.metadata.get("step_id").cloned(),
        continuations: event
            .metadata
            .get("workflow_continuations")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default(),
        recoverable: completed_steps < total_steps
            && event.summary != "Collaboration workflow checkpoint finalized",
        step_status,
    })
}

pub(crate) fn permission_audit(record: PermissionAuditRecord) -> PermissionAudit {
    let resolution = record.resolution;
    PermissionAudit {
        id: record.request.id.0,
        risk: permission_risk_label(&record.request.risk).to_string(),
        action: redact_sensitive_text(&record.request.action),
        reason: redact_sensitive_text(&record.request.reason),
        scope: redact_sensitive_text(&record.request.scope),
        status: if resolution.is_some() {
            "resolved".to_string()
        } else {
            "pending".to_string()
        },
        decision: resolution
            .as_ref()
            .map(|resolution| permission_decision_label(&resolution.decision).to_string()),
        requested_at_ms: record.requested_at_ms,
        resolved_at_ms: resolution.map(|resolution| resolution.resolved_at_ms),
    }
}

pub(crate) fn message_view_from_event(event: &Event) -> Option<ChatMessageView> {
    if event.kind != EventKind::MessageAdded {
        return None;
    }
    if event.metadata.get("internal").map(String::as_str) == Some("true") {
        return None;
    }

    let role = event.metadata.get("role")?.to_string();
    let mut content = redact_sensitive_text(
        event
            .metadata
            .get("display_content")
            .or_else(|| event.metadata.get("content"))?,
    );
    if role == "assistant" {
        content = sanitize_assistant_content(&content);
    }

    Some(ChatMessageView {
        sequence: event.sequence,
        role,
        content,
        timestamp_ms: event.timestamp_ms,
        run_id: event.metadata.get("agent_run_id").cloned(),
        queue_id: event.metadata.get("queue_id").cloned(),
        attachments: attachment_views_from_event(event),
    })
}

pub(crate) fn attachment_views_from_event(event: &Event) -> Vec<AgentAttachmentView> {
    let paths = event
        .metadata
        .get("attachment_paths")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    if paths.is_empty() {
        return Vec::new();
    }

    let names = event
        .metadata
        .get("attachment_names")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let ids = event
        .metadata
        .get("attachment_ids")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let mime_types = event
        .metadata
        .get("attachment_mime_types")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let sizes = event
        .metadata
        .get("attachment_sizes")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let image_paths = event
        .metadata
        .get("image_paths")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();

    paths
        .into_iter()
        .enumerate()
        .filter(|(_, path)| !path.trim().is_empty())
        .map(|(index, path)| {
            let inferred_mime = normalized_attachment_mime("", Path::new(path));
            let mime_type = mime_types
                .get(index)
                .filter(|value| !value.trim().is_empty())
                .map(|value| (*value).to_string())
                .unwrap_or_else(|| {
                    if image_paths.contains(&path) && !inferred_mime.starts_with("image/") {
                        "image/*".to_string()
                    } else {
                        inferred_mime
                    }
                });
            AgentAttachmentView {
                id: ids
                    .get(index)
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| (*value).to_string())
                    .unwrap_or_else(|| format!("message-attachment-{}-{index}", event.sequence)),
                name: names
                    .get(index)
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| (*value).to_string())
                    .unwrap_or_else(|| safe_attachment_name(path)),
                path: path.to_string(),
                mime_type,
                size_bytes: sizes
                    .get(index)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or_default(),
            }
        })
        .collect()
}

pub(crate) fn message_from_event(event: &Event) -> Option<Message> {
    if event.kind != EventKind::MessageAdded {
        return None;
    }
    if event.metadata.get("internal").map(String::as_str) == Some("true")
        && event.metadata.get("kind").map(String::as_str) != Some("visual_reference")
    {
        return None;
    }

    let role = message_role_from_label(event.metadata.get("role")?)?;
    let mut content = redact_sensitive_text(event.metadata.get("content")?);
    if role == MessageRole::Assistant {
        content = sanitize_assistant_content(&content);
    }

    let mut metadata = redact_metadata(&event.metadata);
    if role == MessageRole::Assistant {
        metadata.insert("content".to_string(), content.clone());
        if metadata.contains_key("display_content") {
            metadata.insert("display_content".to_string(), content.clone());
        }
    }

    Some(Message {
        role,
        content,
        metadata,
    })
}

pub(crate) fn tool_run_from_event(event: &Event) -> Option<ToolRunView> {
    if event.kind != EventKind::ToolCallFinished {
        return None;
    }

    Some(ToolRunView {
        invocation_id: event.metadata.get("tool_call_id")?.to_string(),
        tool_name: event.metadata.get("tool")?.to_string(),
        status: event.metadata.get("status")?.to_string(),
        output: event
            .metadata
            .get("output")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        timestamp_ms: event.timestamp_ms,
    })
}

pub(crate) fn tool_approval_from_audit(record: PermissionAuditRecord) -> Option<ToolApprovalView> {
    Some(ToolApprovalView {
        request_id: record.request.id.0,
        invocation_id: record.request.metadata.get("tool_call_id")?.to_string(),
        tool_name: record.request.metadata.get("tool_name")?.to_string(),
        risk: permission_risk_label(&record.request.risk).to_string(),
        reason: redact_sensitive_text(&record.request.reason),
        scope: redact_sensitive_text(&record.request.scope),
        input: record
            .request
            .metadata
            .get("tool_input")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        requested_at_ms: record.requested_at_ms,
    })
}

pub(crate) fn orchestration_step_from_event(event: &Event) -> Option<OrchestrationStepView> {
    if event.kind != EventKind::ModelRequestFinished {
        return None;
    }
    let orchestration_id = event.metadata.get("orchestration_id")?.to_string();

    Some(OrchestrationStepView {
        orchestration_id,
        policy: event.metadata.get("policy")?.to_string(),
        step_index: event.metadata.get("step_index")?.parse().ok()?,
        role: event.metadata.get("role")?.to_string(),
        model: event.metadata.get("model")?.to_string(),
        output: event
            .metadata
            .get("output")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        latency_ms: event
            .metadata
            .get("latency_ms")
            .and_then(|value| value.parse().ok()),
        timestamp_ms: event.timestamp_ms,
    })
}

#[derive(Debug)]
pub(crate) struct ParallelRetrievalResult {
    pub(crate) results: Vec<RagSearchResult>,
    pub(crate) sources: Vec<RagSourceView>,
    pub(crate) trace: RetrievalTraceView,
}

#[derive(Debug)]
pub(crate) struct AutomaticKnowledgeIndexResult {
    pub(crate) stats: RagIndexStats,
    pub(crate) embedding_backend: String,
    pub(crate) embedding_model: String,
    pub(crate) fallback_error: Option<String>,
}
