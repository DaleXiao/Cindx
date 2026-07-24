use agent_core::{
    Event, EventKind, Metadata, ToolCallId, ToolContent, ToolFailure, ToolInvocation,
    ToolOutcomeStatus, ToolResult,
};
use agent_runtime::{
    decode_persisted_tool_artifacts, recovery_source_scope_matches,
    supports_recovery_effect_replay, tool_execution_scope_matches,
};
pub(super) use agent_runtime::{
    finalize_tool_result, tool_input_fingerprint, tool_invocation_context,
    tool_invocation_event_metadata,
};
use agent_storage::{SqliteStore, StorageError};
use std::{
    fs,
    path::{Component, Path},
};
use tools::ToolError;

pub(super) fn failed_tool_result(invocation_id: ToolCallId, error: ToolError) -> ToolResult {
    let mut result = ToolResult::failed(invocation_id, error.message.clone());
    result.failure = Some(ToolFailure {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    });
    result
}

pub(super) fn completed_tool_result(
    store: &SqliteStore,
    invocation: &ToolInvocation,
    workspace_root: &Path,
) -> Result<Option<ToolResult>, StorageError> {
    let events = store.list_by_task_and_tool_call_id(&invocation.task_id, &invocation.id.0)?;
    if let Some(result) = completed_tool_result_from_events(&events, invocation) {
        return Ok(Some(result));
    }

    if !supports_recovery_effect_replay(invocation) {
        return Ok(None);
    }
    let fingerprint = tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
    let effect_events =
        store.list_by_task_and_effect_fingerprint(&invocation.task_id, &fingerprint)?;
    Ok(completed_recovery_effect_from_events(
        &effect_events,
        invocation,
        workspace_root,
    ))
}

fn completed_tool_result_from_events(
    events: &[Event],
    invocation: &ToolInvocation,
) -> Option<ToolResult> {
    let fingerprint = tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
    events.iter().rev().find_map(|event| {
        if event.kind != EventKind::ToolCallFinished
            || event.metadata.get("tool").map(String::as_str) != Some(invocation.tool_name.as_str())
            || event
                .metadata
                .get("result_input_fingerprint")
                .map(String::as_str)
                != Some(fingerprint.as_str())
            || !tool_execution_scope_matches(&event.metadata, invocation)
        {
            return None;
        }
        result_from_finished_event(event, invocation, "exact_call_id")
    })
}

fn completed_recovery_effect_from_events(
    events: &[Event],
    invocation: &ToolInvocation,
    workspace_root: &Path,
) -> Option<ToolResult> {
    let fingerprint = tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
    events.iter().rev().find_map(|event| {
        if event.kind != EventKind::ToolCallFinished
            || event.metadata.get("tool").map(String::as_str) != Some(invocation.tool_name.as_str())
            || event
                .metadata
                .get("result_input_fingerprint")
                .map(String::as_str)
                != Some(fingerprint.as_str())
            || !recovery_source_scope_matches(&event.metadata, invocation)
            || !deterministic_effect_is_still_applied(event, invocation, workspace_root)
        {
            return None;
        }
        let mut result =
            result_from_finished_event(event, invocation, "recovery_source_fingerprint")?;
        result
            .metadata
            .insert("effect_ledger_replay".to_string(), "true".to_string());
        Some(result)
    })
}

fn deterministic_effect_is_still_applied(
    _event: &Event,
    invocation: &ToolInvocation,
    workspace_root: &Path,
) -> bool {
    if invocation.tool_name != "file.write" {
        return false;
    }
    let Ok(input) = serde_json::from_str::<serde_json::Value>(&invocation.input_json) else {
        return false;
    };
    let Some(path) = input.get("path").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let Some(content) = input.get("content").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return false;
    }
    let Ok(canonical_root) = fs::canonicalize(workspace_root) else {
        return false;
    };
    let candidate = workspace_root.join(relative);
    let Ok(canonical_candidate) = fs::canonicalize(candidate) else {
        return false;
    };
    canonical_candidate.starts_with(&canonical_root)
        && fs::read(canonical_candidate).is_ok_and(|bytes| bytes.as_slice() == content.as_bytes())
}

fn result_from_finished_event(
    event: &Event,
    invocation: &ToolInvocation,
    replay_mode: &str,
) -> Option<ToolResult> {
    let status = match event.metadata.get("status").map(String::as_str)? {
        "succeeded" => ToolOutcomeStatus::Succeeded,
        "failed" => ToolOutcomeStatus::Failed,
        _ => return None,
    };
    let retryable = event
        .metadata
        .get("result_failure_retryable")
        .and_then(|value| value.parse::<bool>().ok());
    if matches!(status, ToolOutcomeStatus::Failed) && retryable != Some(false) {
        return None;
    }

    let output = event.metadata.get("output").cloned().unwrap_or_default();
    let mut metadata = event
        .metadata
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("result_")
                .map(|key| (key.to_string(), value.clone()))
        })
        .collect::<Metadata>();
    metadata.insert("idempotent_replay".to_string(), "true".to_string());
    metadata.insert("effect_replay_mode".to_string(), replay_mode.to_string());
    metadata.insert("replayed_event_id".to_string(), event.id.0.clone());
    metadata.insert(
        "replayed_event_sequence".to_string(),
        event.sequence.to_string(),
    );
    if event.metadata.get("output_omitted").map(String::as_str) == Some("true") {
        metadata.insert("replayed_output_compacted".to_string(), "true".to_string());
    }

    let artifacts = decode_persisted_tool_artifacts(&metadata);
    let failure = matches!(status, ToolOutcomeStatus::Failed).then(|| ToolFailure {
        code: metadata
            .get("failure_code")
            .cloned()
            .unwrap_or_else(|| "tool_execution_failed".to_string()),
        message: output.clone(),
        retryable: false,
    });
    let structured_output_json = metadata.get("structured_output").cloned();

    Some(ToolResult {
        invocation_id: ToolCallId(invocation.id.0.clone()),
        status,
        output: output.clone(),
        content: vec![ToolContent::Text(output)],
        structured_output_json,
        artifacts,
        failure,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, TaskId, ToolArtifact};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn invocation(input_json: &str) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId("call-1".to_string()),
            task_id: TaskId("task-1".to_string()),
            tool_name: "filesystem.write".to_string(),
            input_json: input_json.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: [("session_id".to_string(), "session-1".to_string())]
                .into_iter()
                .collect(),
        }
    }

    fn finished_event(invocation: &ToolInvocation, result: &ToolResult) -> Event {
        let mut metadata = tool_invocation_event_metadata(invocation);
        metadata.insert("status".to_string(), "succeeded".to_string());
        metadata.insert("output".to_string(), result.output.clone());
        metadata.insert("session_id".to_string(), "session-1".to_string());
        for (key, value) in &result.metadata {
            metadata.insert(format!("result_{key}"), value.clone());
        }
        Event {
            id: EventId("event-1".to_string()),
            task_id: invocation.task_id.clone(),
            sequence: 7,
            timestamp_ms: 1,
            kind: EventKind::ToolCallFinished,
            summary: "finished".to_string(),
            metadata,
        }
    }

    fn temporary_workspace(name: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cindx-{name}-{suffix}"));
        fs::create_dir_all(&root).expect("temporary workspace should be created");
        root
    }

    fn recovery_file_write_invocation(
        call_id: &str,
        agent_run_id: &str,
        input_json: &str,
    ) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId(call_id.to_string()),
            task_id: TaskId("task-1".to_string()),
            tool_name: "file.write".to_string(),
            input_json: input_json.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: [
                ("project_id".to_string(), "project-1".to_string()),
                ("session_id".to_string(), "session-1".to_string()),
                ("agent_run_id".to_string(), agent_run_id.to_string()),
            ]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn fingerprint_is_stable_for_equivalent_json_objects() {
        assert_eq!(
            tool_input_fingerprint("tool", r#"{"b":2,"a":1}"#),
            tool_input_fingerprint("tool", r#"{"a":1,"b":2}"#)
        );
    }

    #[test]
    fn completed_result_requires_the_exact_input_and_scope() {
        let invocation = invocation(r#"{"path":"a.txt"}"#);
        let mut result = ToolResult::text(
            invocation.id.clone(),
            ToolOutcomeStatus::Succeeded,
            "written",
            Metadata::new(),
        );
        let fingerprint = tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
        finalize_tool_result(
            &mut result,
            &invocation.id,
            &fingerprint,
            Duration::from_millis(12),
        );
        let event = finished_event(&invocation, &result);

        assert!(completed_tool_result_from_events(&[event.clone()], &invocation).is_some());
        assert!(completed_tool_result_from_events(
            &[event.clone()],
            &super::tests::invocation(r#"{"path":"b.txt"}"#)
        )
        .is_none());

        let mut other_session = invocation.clone();
        other_session
            .metadata
            .insert("session_id".to_string(), "session-2".to_string());
        assert!(completed_tool_result_from_events(&[event], &other_session).is_none());
    }

    #[test]
    fn completed_result_rehydrates_artifacts_and_replay_metadata() {
        let invocation = invocation(r#"{"path":"a.txt"}"#);
        let mut result = ToolResult::text(
            invocation.id.clone(),
            ToolOutcomeStatus::Succeeded,
            "written",
            Metadata::new(),
        );
        result.artifacts.push(ToolArtifact {
            path: "/tmp/a.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            title: Some("Output".to_string()),
        });
        let fingerprint = tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
        finalize_tool_result(
            &mut result,
            &invocation.id,
            &fingerprint,
            Duration::from_millis(12),
        );
        let replayed =
            completed_tool_result_from_events(&[finished_event(&invocation, &result)], &invocation)
                .expect("completed result should replay");

        assert_eq!(replayed.output, "written");
        assert_eq!(replayed.artifacts, result.artifacts);
        assert_eq!(
            replayed
                .metadata
                .get("idempotent_replay")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn exact_call_replay_accepts_the_recovery_source_run() {
        let original = recovery_file_write_invocation(
            "call-1",
            "run-source",
            r#"{"path":"a.txt","content":"written"}"#,
        );
        let mut result = ToolResult::text(
            original.id.clone(),
            ToolOutcomeStatus::Succeeded,
            "written",
            Metadata::new(),
        );
        let fingerprint = tool_input_fingerprint(&original.tool_name, &original.input_json);
        finalize_tool_result(
            &mut result,
            &original.id,
            &fingerprint,
            Duration::from_millis(12),
        );
        let event = finished_event(&original, &result);
        let mut recovered = original.clone();
        recovered
            .metadata
            .insert("agent_run_id".to_string(), "run-new".to_string());
        recovered
            .metadata
            .insert("source_agent_run_id".to_string(), "run-source".to_string());

        let replayed = completed_tool_result_from_events(&[event], &recovered)
            .expect("the exact logical call should replay across a recovery run");
        assert_eq!(
            replayed
                .metadata
                .get("effect_replay_mode")
                .map(String::as_str),
            Some("exact_call_id")
        );
    }

    #[test]
    fn recovery_effect_ledger_replays_only_a_verified_file_write() {
        let root = temporary_workspace("effect-ledger");
        fs::write(root.join("a.txt"), "written").expect("effect should exist");
        let original = recovery_file_write_invocation(
            "call-1",
            "run-source",
            r#"{"path":"a.txt","content":"written"}"#,
        );
        let mut result = ToolResult::text(
            original.id.clone(),
            ToolOutcomeStatus::Succeeded,
            "file written",
            Metadata::new(),
        );
        let fingerprint = tool_input_fingerprint(&original.tool_name, &original.input_json);
        finalize_tool_result(
            &mut result,
            &original.id,
            &fingerprint,
            Duration::from_millis(12),
        );
        let event = finished_event(&original, &result);
        let mut recovered = recovery_file_write_invocation(
            "call-2",
            "run-new",
            r#"{"content":"written","path":"a.txt"}"#,
        );
        recovered
            .metadata
            .insert("source_agent_run_id".to_string(), "run-source".to_string());
        recovered
            .metadata
            .insert("recovery_resume_key".to_string(), "resume-1".to_string());

        let replayed = completed_recovery_effect_from_events(&[event.clone()], &recovered, &root)
            .expect("verified deterministic effect should replay");
        assert_eq!(
            replayed
                .metadata
                .get("effect_replay_mode")
                .map(String::as_str),
            Some("recovery_source_fingerprint")
        );
        assert_eq!(
            replayed
                .metadata
                .get("effect_ledger_replay")
                .map(String::as_str),
            Some("true")
        );

        fs::write(root.join("a.txt"), "changed").expect("effect should be changed");
        assert!(completed_recovery_effect_from_events(&[event], &recovered, &root).is_none());
        fs::remove_dir_all(root).expect("temporary workspace should be removed");
    }

    #[test]
    fn recovery_effect_ledger_rejects_cross_run_and_non_recovery_reuse() {
        let root = temporary_workspace("effect-scope");
        fs::write(root.join("a.txt"), "written").expect("effect should exist");
        let original = recovery_file_write_invocation(
            "call-1",
            "run-source",
            r#"{"path":"a.txt","content":"written"}"#,
        );
        let mut result = ToolResult::text(
            original.id.clone(),
            ToolOutcomeStatus::Succeeded,
            "file written",
            Metadata::new(),
        );
        let fingerprint = tool_input_fingerprint(&original.tool_name, &original.input_json);
        finalize_tool_result(
            &mut result,
            &original.id,
            &fingerprint,
            Duration::from_millis(12),
        );
        let event = finished_event(&original, &result);
        let mut recovered = recovery_file_write_invocation(
            "call-2",
            "run-new",
            r#"{"path":"a.txt","content":"written"}"#,
        );
        assert!(!supports_recovery_effect_replay(&recovered));
        recovered.metadata.insert(
            "source_agent_run_id".to_string(),
            "different-source".to_string(),
        );
        recovered
            .metadata
            .insert("recovery_resume_key".to_string(), "resume-1".to_string());
        assert!(completed_recovery_effect_from_events(&[event], &recovered, &root).is_none());
        fs::remove_dir_all(root).expect("temporary workspace should be removed");
    }

    #[test]
    fn retryable_failure_is_not_replayed() {
        let invocation = invocation(r#"{"path":"a.txt"}"#);
        let mut result = ToolResult::text(
            invocation.id.clone(),
            ToolOutcomeStatus::Failed,
            "temporary failure",
            Metadata::new(),
        );
        result.failure = Some(ToolFailure {
            code: "temporary".to_string(),
            message: "temporary failure".to_string(),
            retryable: true,
        });
        let fingerprint = tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
        finalize_tool_result(
            &mut result,
            &invocation.id,
            &fingerprint,
            Duration::from_millis(12),
        );
        let mut event = finished_event(&invocation, &result);
        event
            .metadata
            .insert("status".to_string(), "failed".to_string());

        assert!(completed_tool_result_from_events(&[event], &invocation).is_none());
    }

    #[test]
    fn tool_error_preserves_structured_retryability() {
        let result = failed_tool_result(
            ToolCallId("call-1".to_string()),
            ToolError::retryable("tool_timeout", "temporary timeout"),
        );

        assert_eq!(result.status, ToolOutcomeStatus::Failed);
        assert_eq!(
            result.failure,
            Some(ToolFailure {
                code: "tool_timeout".to_string(),
                message: "temporary timeout".to_string(),
                retryable: true,
            })
        );
    }
}
