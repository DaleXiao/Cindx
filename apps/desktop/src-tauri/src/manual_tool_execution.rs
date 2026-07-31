use crate::{
    append_event, append_tool_finished_event, completed_tool_result, failed_tool_result,
    finalize_tool_result, metadata_with_context, tool_input_fingerprint, tool_invocation_context,
    tool_invocation_event_metadata, tool_outcome_label,
};
use agent_core::{EventKind, Metadata, TaskId, ToolCallId, ToolInvocation, ToolResult};
use agent_runtime::apply_tool_spec_runtime_metadata;
use agent_storage::{SqliteStore, StorageError};
use std::{path::Path, sync::Mutex, time::Instant};
use tools::ToolRegistry;

enum ManualToolPreparation {
    Completed(ToolResult),
    Execute(PreparedManualToolExecution),
}

struct PreparedManualToolExecution {
    invocation: ToolInvocation,
    task_id: TaskId,
    tool_call_id: String,
    tool_name: String,
    input_fingerprint: String,
    event_context: Option<Metadata>,
    started_at: Instant,
}

struct CompletedManualToolExecution {
    task_id: TaskId,
    tool_call_id: String,
    tool_name: String,
    event_context: Option<Metadata>,
    result: ToolResult,
}

pub(crate) fn execute_manual_tool_invocation(
    execution_gate: &Mutex<()>,
    store: &Mutex<SqliteStore>,
    registry: &ToolRegistry,
    invocation: ToolInvocation,
    workspace_root: &Path,
    run_context: Option<&Metadata>,
) -> Result<ToolResult, String> {
    let _execution_gate = execution_gate
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let preparation = {
        let mut store = store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        prepare_manual_tool_execution(
            &mut store,
            registry,
            invocation,
            workspace_root,
            run_context,
        )
        .map_err(|error| error.to_string())?
    };
    let prepared = match preparation {
        ManualToolPreparation::Completed(result) => return Ok(result),
        ManualToolPreparation::Execute(prepared) => prepared,
    };

    let completed = perform_manual_tool_execution(registry, prepared);
    let mut store = store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    commit_manual_tool_execution(&mut store, completed).map_err(|error| error.to_string())
}

fn prepare_manual_tool_execution(
    store: &mut SqliteStore,
    registry: &ToolRegistry,
    mut invocation: ToolInvocation,
    workspace_root: &Path,
    run_context: Option<&Metadata>,
) -> Result<ManualToolPreparation, StorageError> {
    if let Some(tool) = registry.get(&invocation.tool_name) {
        let effect_spec = tool.effect_spec(&invocation);
        apply_tool_spec_runtime_metadata(&mut invocation, &effect_spec);
    }
    if let Some(result) = completed_tool_result(store, &invocation, workspace_root)? {
        return Ok(ManualToolPreparation::Completed(result));
    }

    let invocation_context = tool_invocation_context(&invocation);
    let event_context = run_context
        .cloned()
        .or_else(|| (!invocation_context.is_empty()).then_some(invocation_context));
    let metadata = match run_context {
        Some(context) => {
            metadata_with_context(tool_invocation_event_metadata(&invocation), context)
        }
        None => tool_invocation_event_metadata(&invocation),
    };
    append_event(
        store,
        &invocation.task_id,
        EventKind::ToolCallStarted,
        format!("Tool call started: {}", invocation.tool_name),
        metadata,
    )?;

    let task_id = invocation.task_id.clone();
    let tool_call_id = invocation.id.0.clone();
    let tool_name = invocation.tool_name.clone();
    let input_fingerprint = tool_input_fingerprint(&tool_name, &invocation.input_json);
    Ok(ManualToolPreparation::Execute(
        PreparedManualToolExecution {
            invocation,
            task_id,
            tool_call_id,
            tool_name,
            input_fingerprint,
            event_context,
            started_at: Instant::now(),
        },
    ))
}

fn perform_manual_tool_execution(
    registry: &ToolRegistry,
    prepared: PreparedManualToolExecution,
) -> CompletedManualToolExecution {
    let PreparedManualToolExecution {
        invocation,
        task_id,
        tool_call_id,
        tool_name,
        input_fingerprint,
        event_context,
        started_at,
    } = prepared;
    let mut result = match registry.get(&tool_name) {
        Some(tool) => tool
            .execute(invocation)
            .unwrap_or_else(|error| failed_tool_result(ToolCallId(tool_call_id.clone()), error)),
        None => ToolResult::failed(ToolCallId(tool_call_id.clone()), "unknown tool"),
    };
    finalize_tool_result(
        &mut result,
        &ToolCallId(tool_call_id.clone()),
        &input_fingerprint,
        started_at.elapsed(),
    );
    CompletedManualToolExecution {
        task_id,
        tool_call_id,
        tool_name,
        event_context,
        result,
    }
}

fn commit_manual_tool_execution(
    store: &mut SqliteStore,
    completed: CompletedManualToolExecution,
) -> Result<ToolResult, StorageError> {
    let result = completed.result;
    append_tool_finished_event(
        store,
        &completed.task_id,
        &completed.tool_call_id,
        &completed.tool_name,
        tool_outcome_label(&result.status),
        &result.output,
        result.metadata.clone(),
        completed.event_context.as_ref(),
    )?;
    Ok(result)
}
