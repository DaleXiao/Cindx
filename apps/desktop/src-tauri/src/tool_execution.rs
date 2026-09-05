use super::*;

pub(crate) fn phase8_state_with_error(
    state: &AppState,
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

pub(crate) fn phase5_state_with_error(
    state: &AppState,
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

pub(crate) enum AgentToolInvocationOutcome {
    Completed(Box<ToolResult>),
    RestartAfterSteer,
}

#[derive(Clone)]
struct AgentToolEpochGuard {
    control: Arc<AgentRunControl>,
    epoch: AgentToolEpoch,
}

#[derive(Clone, Copy)]
enum AgentToolEpoch {
    Execution(agent_runtime::RunEpochLease),
    Objective(u64),
}

impl AgentToolEpochGuard {
    fn epoch(&self) -> u64 {
        match self.epoch {
            AgentToolEpoch::Execution(lease) => lease.epoch(),
            AgentToolEpoch::Objective(epoch) => epoch,
        }
    }

    fn is_current(&self) -> bool {
        match self.epoch {
            AgentToolEpoch::Execution(lease) => {
                self.control.execution_epoch_lease_is_current(lease)
            }
            AgentToolEpoch::Objective(epoch) => self.control.objective_epoch_is_current(epoch),
        }
    }

    fn begin_tool_call(
        &self,
        scope: &str,
        tool_name: &str,
        input: &str,
    ) -> agent_runtime::RunToolCallStart {
        match self.epoch {
            AgentToolEpoch::Execution(lease) => self
                .control
                .begin_tool_call_with_epoch(lease, scope, tool_name, input),
            AgentToolEpoch::Objective(epoch) => self
                .control
                .begin_tool_call_at(epoch, scope, tool_name, input),
        }
    }
}

pub(crate) fn execute_agent_tool_invocation_for_epoch(
    state: &AppState,
    registry: &ToolRegistry,
    invocation: ToolInvocation,
    workspace_root: &Path,
    run_context: &Metadata,
    control: &Arc<AgentRunControl>,
    lease: agent_runtime::RunEpochLease,
) -> Result<AgentToolInvocationOutcome, String> {
    execute_agent_tool_invocation_inner(
        state,
        registry,
        invocation,
        workspace_root,
        run_context,
        Some(AgentToolEpochGuard {
            control: Arc::clone(control),
            epoch: AgentToolEpoch::Execution(lease),
        }),
    )
}

pub(crate) fn execute_agent_tool_invocation_for_objective_epoch(
    state: &AppState,
    registry: &ToolRegistry,
    invocation: ToolInvocation,
    workspace_root: &Path,
    run_context: &Metadata,
    control: &Arc<AgentRunControl>,
    expected_epoch: u64,
) -> Result<AgentToolInvocationOutcome, String> {
    execute_agent_tool_invocation_inner(
        state,
        registry,
        invocation,
        workspace_root,
        run_context,
        Some(AgentToolEpochGuard {
            control: Arc::clone(control),
            epoch: AgentToolEpoch::Objective(expected_epoch),
        }),
    )
}

fn execute_agent_tool_invocation_inner(
    state: &AppState,
    registry: &ToolRegistry,
    mut invocation: ToolInvocation,
    workspace_root: &Path,
    run_context: &Metadata,
    epoch_guard: Option<AgentToolEpochGuard>,
) -> Result<AgentToolInvocationOutcome, String> {
    for (key, value) in run_context {
        invocation
            .metadata
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    // Confinement is session policy, not tool input: right before a shell
    // execution, resolve the session's effective sandbox mode from the live
    // event log and hand it to the tool through invocation metadata. Absent
    // sessions or mode events keep the historical unconfined argv.
    if invocation.tool_name == "shell.run" {
        let sandbox_mode =
            crate::sandbox_mode_runtime::effective_sandbox_mode_before_tool_execution(
                state,
                run_context,
            )?;
        invocation.metadata.insert(
            agent_core::SANDBOX_MODE_METADATA_KEY.to_string(),
            sandbox_mode.label().to_string(),
        );
    }
    if let Some(tool) = registry.get(&invocation.tool_name) {
        let effect_spec = tool.effect_spec(&invocation);
        agent_runtime::apply_tool_spec_runtime_metadata(&mut invocation, &effect_spec);
    }
    let task_id = invocation.task_id.clone();
    let tool_call_id = invocation.id.0.clone();
    let tool_name = invocation.tool_name.clone();
    let tool_input = invocation.input_json.clone();
    let input_fingerprint = tool_input_fingerprint(&tool_name, &tool_input);
    let run_control = match epoch_guard.as_ref() {
        Some(guard) => Some(Arc::clone(&guard.control)),
        None => active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?,
    };
    let completed_result = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        completed_tool_result(&store, &invocation, workspace_root)
            .map_err(|error| error.to_string())?
    };
    if let Some(result) = completed_result {
        if epoch_guard
            .as_ref()
            .is_some_and(|guard| !guard.is_current())
        {
            return Ok(AgentToolInvocationOutcome::RestartAfterSteer);
        }
        return Ok(AgentToolInvocationOutcome::Completed(Box::new(result)));
    }

    let execution_started_at = Instant::now();
    let scope = run_context
        .get("stage")
        .or_else(|| run_context.get("collaboration_stage"))
        .map(String::as_str)
        .unwrap_or("executor");
    let (budget_stop, tool_started) = match (run_control.as_ref(), epoch_guard.as_ref()) {
        (Some(_), Some(guard)) => {
            match guard.begin_tool_call(scope, &tool_name, &invocation.input_json) {
                agent_runtime::RunToolCallStart::Started(_) => (None, true),
                agent_runtime::RunToolCallStart::RestartAfterSteer
                | agent_runtime::RunToolCallStart::TerminalCommitted => {
                    return Ok(AgentToolInvocationOutcome::RestartAfterSteer)
                }
                agent_runtime::RunToolCallStart::Stopped(reason) => (Some(reason), false),
            }
        }
        (Some(control), None) => {
            match control.begin_tool_call(scope, &tool_name, &invocation.input_json) {
                Ok(_) => (None, true),
                Err(reason) => (Some(reason), false),
            }
        }
        (None, _) => (None, false),
    };
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &task_id,
            EventKind::ToolCallStarted,
            format!("Tool call started: {tool_name}"),
            metadata_with_context(tool_invocation_event_metadata(&invocation), run_context),
        )
        .map_err(|error| error.to_string())?;
    }
    let tool_control = ToolExecutionControl::new({
        let run_control = run_control.clone();
        let epoch_guard = epoch_guard.clone();
        move || {
            run_control
                .as_ref()
                .is_some_and(|control| control.should_stop())
                || epoch_guard
                    .as_ref()
                    .is_some_and(|guard| !guard.is_current())
        }
    });
    let mutates_workspace = registry
        .get(&tool_name)
        .is_some_and(|tool| tool_may_mutate_workspace(&tool_name, &tool.spec().risk));
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
    let stale_epoch = epoch_guard
        .as_ref()
        .is_some_and(|guard| !guard.is_current());
    if let Some(control) = run_control.as_ref() {
        if tool_started {
            if let Some(guard) = epoch_guard.as_ref() {
                control.finish_tool_call_at(guard.epoch());
            } else {
                control.finish_tool_call();
            }
        }
        let objective_epoch = epoch_guard
            .as_ref()
            .map(AgentToolEpochGuard::epoch)
            .unwrap_or_else(|| run_context_steer_epoch(run_context));
        control.mark_progress_at(objective_epoch, "tool_result", &tool_name);
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
    if stale_epoch {
        result
            .metadata
            .insert("superseded_by_steer".to_string(), "true".to_string());
    }
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
    if stale_epoch {
        Ok(AgentToolInvocationOutcome::RestartAfterSteer)
    } else {
        Ok(AgentToolInvocationOutcome::Completed(Box::new(result)))
    }
}

pub(crate) fn tool_may_mutate_workspace(tool_name: &str, risk: &ToolRisk) -> bool {
    if tool_name.starts_with("browser.") || tool_name.starts_with("computer.") {
        return false;
    }
    !matches!(risk, ToolRisk::ReadOnly)
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
        crate::private_files::private_dir_ensure(workspace_root, &output_dir)
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
        crate::private_files::private_file_write(workspace_root, &path, &bytes)
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
            crate::private_files::private_dir_ensure(workspace_root, &output_dir)
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
        crate::private_files::private_dir_ensure(workspace_root, parent)
            .map_err(|error| format!("failed to create output history directory: {error}"))?;
    }
    // `fs::copy` follows a destination symlink; reject a planted leaf before the
    // copy so a hostile workspace cannot redirect the snapshot outside the root.
    if fs::symlink_metadata(&snapshot)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "refusing to preserve tool output through a symbolic link: {}",
            snapshot.display()
        ));
    }
    fs::copy(&source, &snapshot)
        .map_err(|error| format!("failed to preserve tool output version: {error}"))?;
    crate::private_files::private_file_secure(workspace_root, &snapshot)
        .map_err(|error| format!("failed to secure tool output version: {error}"))?;
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
    state: &AppState,
    config: &ProviderConfig,
    message: &str,
) -> Result<Phase4State, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase4_state(&mut store, config, Some(message.to_string())).map_err(|error| error.to_string())
}

pub(crate) fn record_phase4_error(state: &AppState, message: &str) -> Result<(), String> {
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
        tool_name: event.metadata.get("tool").cloned(),
        state: event_state(&event.kind, permission_is_pending).to_string(),
        timestamp_ms: event.timestamp_ms,
    }
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

#[path = "tool_execution_event_projection.rs"]
mod event_projection_helpers;
pub(crate) use event_projection_helpers::*;

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
