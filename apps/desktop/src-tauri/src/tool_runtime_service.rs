use agent_core::{
    Event, EventKind, Metadata, ToolCallId, ToolContent, ToolFailure, ToolInvocation,
    ToolOutcomeStatus, ToolResult,
};
use agent_runtime::{
    decode_persisted_tool_artifacts, decode_persisted_tool_model_observation,
    recovery_source_scope_matches, tool_effect_recovery_policy, tool_execution_scope_matches,
    ToolEffectRecoveryPolicy, TOOL_EFFECT_VERIFIER_METADATA_KEY,
};
pub(super) use agent_runtime::{
    finalize_tool_result, tool_input_fingerprint, tool_invocation_context,
    tool_invocation_event_metadata,
};
use agent_storage::{SqliteStore, StorageError};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
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

    if let Some(started) = interrupted_tool_call_started_event(&events, invocation) {
        let recovery_invocation = invocation_with_recorded_effect_contract(invocation, started);
        return Ok(match tool_effect_recovery_policy(&recovery_invocation) {
            ToolEffectRecoveryPolicy::SafeToRetry => None,
            ToolEffectRecoveryPolicy::VerifyBeforeRetry
                if deterministic_effect_is_still_applied(&recovery_invocation, workspace_root) =>
            {
                Some(verified_interrupted_effect_result(&recovery_invocation))
            }
            ToolEffectRecoveryPolicy::VerifyBeforeRetry
            | ToolEffectRecoveryPolicy::NeverRetryUnknown => {
                Some(unknown_interrupted_effect_result(&recovery_invocation))
            }
        });
    }

    if !has_recovery_effect_replay_lineage(invocation) {
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

pub(super) fn completed_exact_tool_result(
    store: &SqliteStore,
    invocation: &ToolInvocation,
) -> Result<Option<ToolResult>, StorageError> {
    let events = store.list_by_task_and_tool_call_id(&invocation.task_id, &invocation.id.0)?;
    let matching_lineage = events
        .into_iter()
        .filter(|event| permission_replay_lineage_matches(&event.metadata, invocation))
        .collect::<Vec<_>>();
    Ok(completed_tool_result_from_events(
        &matching_lineage,
        invocation,
    ))
}

fn permission_replay_lineage_matches(
    event_metadata: &Metadata,
    invocation: &ToolInvocation,
) -> bool {
    let Some(expected) = invocation
        .metadata
        .get("prompt_contract_epoch")
        .or_else(|| invocation.metadata.get("steer_epoch"))
    else {
        return false;
    };
    event_metadata
        .get("prompt_contract_epoch")
        .or_else(|| event_metadata.get("steer_epoch"))
        == Some(expected)
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
        let recorded_invocation = invocation_with_recorded_effect_contract(invocation, event);
        if event.kind != EventKind::ToolCallFinished
            || event.metadata.get("tool").map(String::as_str) != Some(invocation.tool_name.as_str())
            || event
                .metadata
                .get("result_input_fingerprint")
                .map(String::as_str)
                != Some(fingerprint.as_str())
            || !recovery_source_scope_matches(&event.metadata, invocation)
            || tool_effect_recovery_policy(&recorded_invocation)
                != ToolEffectRecoveryPolicy::VerifyBeforeRetry
            || !deterministic_effect_is_still_applied(&recorded_invocation, workspace_root)
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

fn has_recovery_effect_replay_lineage(invocation: &ToolInvocation) -> bool {
    invocation
        .metadata
        .get("source_agent_run_id")
        .is_some_and(|value| !value.trim().is_empty())
        && invocation
            .metadata
            .get("recovery_resume_key")
            .is_some_and(|value| !value.trim().is_empty())
}

fn deterministic_effect_is_still_applied(
    invocation: &ToolInvocation,
    workspace_root: &Path,
) -> bool {
    let verifier = invocation
        .metadata
        .get(TOOL_EFFECT_VERIFIER_METADATA_KEY)
        .map(String::as_str)
        .or_else(|| (invocation.tool_name == "file.write").then_some("workspace_file_content_v1"));
    match verifier {
        Some("workspace_file_content_v1") => {
            workspace_file_content_matches(invocation, workspace_root)
        }
        Some(verifier) => verifier
            .strip_prefix("workspace_file_sha256_v1:")
            .is_some_and(|expected| {
                workspace_file_sha256_matches(invocation, workspace_root, expected)
            }),
        _ => false,
    }
}

fn workspace_file_sha256_matches(
    invocation: &ToolInvocation,
    workspace_root: &Path,
    expected_sha256: &str,
) -> bool {
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }
    let Some(path) = workspace_file_effect_path(invocation, "file.patch") else {
        return false;
    };
    let Some(candidate) = canonical_workspace_file(workspace_root, &path) else {
        return false;
    };
    let Ok(metadata) = fs::metadata(&candidate) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
        return false;
    }
    let Ok(mut file) = fs::File::open(candidate) else {
        return false;
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(expected_sha256)
}

fn workspace_file_content_matches(invocation: &ToolInvocation, workspace_root: &Path) -> bool {
    let Ok(input) = serde_json::from_str::<serde_json::Value>(&invocation.input_json) else {
        return false;
    };
    let effect_input = if invocation.tool_name == "tool.invoke" {
        if input.get("name").and_then(serde_json::Value::as_str) != Some("file.write") {
            return false;
        }
        let Some(arguments) = input
            .get("arguments")
            .filter(|arguments| arguments.is_object())
        else {
            return false;
        };
        arguments
    } else {
        &input
    };
    let Some(path) = effect_input.get("path").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let Some(content) = effect_input
        .get("content")
        .and_then(serde_json::Value::as_str)
    else {
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

fn workspace_file_effect_path(invocation: &ToolInvocation, expected_tool: &str) -> Option<String> {
    let input = serde_json::from_str::<serde_json::Value>(&invocation.input_json).ok()?;
    let effect_input = if invocation.tool_name == "tool.invoke" {
        (input.get("name")?.as_str()? == expected_tool)
            .then_some(input.get("arguments")?.as_object()?)?
    } else {
        (invocation.tool_name == expected_tool).then_some(input.as_object()?)?
    };
    effect_input.get("path")?.as_str().map(str::to_string)
}

pub(crate) fn canonical_workspace_file(
    workspace_root: &Path,
    path: &str,
) -> Option<std::path::PathBuf> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    let canonical_root = fs::canonicalize(workspace_root).ok()?;
    let canonical_candidate = fs::canonicalize(workspace_root.join(relative)).ok()?;
    canonical_candidate
        .starts_with(&canonical_root)
        .then_some(canonical_candidate)
}

fn interrupted_tool_call_started_event<'a>(
    events: &'a [Event],
    invocation: &ToolInvocation,
) -> Option<&'a Event> {
    let fingerprint = tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
    let matches = |event: &Event| {
        event.metadata.get("tool").map(String::as_str) == Some(invocation.tool_name.as_str())
            && event
                .metadata
                .get("input_fingerprint")
                .or_else(|| event.metadata.get("result_input_fingerprint"))
                .map(String::as_str)
                == Some(fingerprint.as_str())
            && tool_execution_scope_matches(&event.metadata, invocation)
    };
    if events
        .iter()
        .any(|event| event.kind == EventKind::ToolCallFinished && matches(event))
    {
        return None;
    }
    events
        .iter()
        .rev()
        .find(|event| event.kind == EventKind::ToolCallStarted && matches(event))
}

fn invocation_with_recorded_effect_contract(
    invocation: &ToolInvocation,
    started: &Event,
) -> ToolInvocation {
    let mut recovered = invocation.clone();
    let keys = [
        agent_runtime::TOOL_RISK_METADATA_KEY,
        agent_runtime::TOOL_EFFECT_SEMANTICS_METADATA_KEY,
        TOOL_EFFECT_VERIFIER_METADATA_KEY,
    ];
    if keys.iter().any(|key| started.metadata.contains_key(*key)) {
        for key in keys {
            recovered.metadata.remove(key);
            if let Some(value) = started.metadata.get(key) {
                recovered.metadata.insert(key.to_string(), value.clone());
            }
        }
    }
    recovered
}

fn verified_interrupted_effect_result(invocation: &ToolInvocation) -> ToolResult {
    let mut result = ToolResult::text(
        invocation.id.clone(),
        ToolOutcomeStatus::Succeeded,
        "The deterministic tool effect was already applied before interruption; Cindx verified the current workspace state and did not repeat it.",
        Metadata::new(),
    );
    result
        .metadata
        .insert("idempotent_replay".to_string(), "true".to_string());
    result.metadata.insert(
        "effect_replay_mode".to_string(),
        "verified_interrupted_effect".to_string(),
    );
    result
}

fn unknown_interrupted_effect_result(invocation: &ToolInvocation) -> ToolResult {
    let message = format!(
        "The previous {} call started but did not record a terminal result. Cindx will not repeat a potentially external side effect automatically. Inspect the current state before proposing a new call.",
        invocation.tool_name
    );
    let mut result = ToolResult::failed(invocation.id.clone(), message.clone());
    result.failure = Some(ToolFailure {
        code: "tool_effect_outcome_unknown".to_string(),
        message,
        retryable: false,
    });
    result.metadata.insert(
        "effect_recovery_mode".to_string(),
        "blocked_unknown_outcome".to_string(),
    );
    result
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
    let model_observation = decode_persisted_tool_model_observation(&metadata);

    Some(ToolResult {
        invocation_id: ToolCallId(invocation.id.0.clone()),
        status,
        output: output.clone(),
        content: vec![ToolContent::Text(output)],
        structured_output_json,
        artifacts,
        failure,
        model_observation,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, TaskId, ToolArtifact, ToolObservationV2};
    use agent_storage::EventStore;
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
                (
                    agent_runtime::TOOL_RISK_METADATA_KEY.to_string(),
                    "writes_workspace".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        }
    }

    fn recovery_meta_file_write_invocation(input_json: &str) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId("call-meta-write".to_string()),
            task_id: TaskId("task-1".to_string()),
            tool_name: "tool.invoke".to_string(),
            input_json: input_json.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: [
                ("project_id".to_string(), "project-1".to_string()),
                ("session_id".to_string(), "session-1".to_string()),
                (
                    agent_runtime::TOOL_EFFECT_VERIFIER_METADATA_KEY.to_string(),
                    "workspace_file_content_v1".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        }
    }

    fn started_event(invocation: &ToolInvocation) -> Event {
        Event {
            id: EventId("event-started".to_string()),
            task_id: invocation.task_id.clone(),
            sequence: 6,
            timestamp_ms: 1,
            kind: EventKind::ToolCallStarted,
            summary: "started".to_string(),
            metadata: tool_invocation_event_metadata(invocation),
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

        assert!(
            completed_tool_result_from_events(std::slice::from_ref(&event), &invocation).is_some()
        );
        assert!(completed_tool_result_from_events(
            std::slice::from_ref(&event),
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
    fn permission_replay_requires_the_same_prompt_contract_epoch() {
        let mut invocation = invocation(r#"{"path":"a.txt"}"#);
        invocation
            .metadata
            .insert("prompt_contract_epoch".to_string(), "3".to_string());
        let mut matching = Metadata::new();
        matching.insert("prompt_contract_epoch".to_string(), "3".to_string());
        assert!(permission_replay_lineage_matches(&matching, &invocation));

        matching.insert("prompt_contract_epoch".to_string(), "4".to_string());
        assert!(!permission_replay_lineage_matches(&matching, &invocation));
        matching.clear();
        assert!(!permission_replay_lineage_matches(&matching, &invocation));
        invocation.metadata.remove("prompt_contract_epoch");
        matching.insert("prompt_contract_epoch".to_string(), "3".to_string());
        assert!(!permission_replay_lineage_matches(&matching, &invocation));
    }

    #[test]
    fn completed_exact_result_is_reused_only_within_its_permission_lineage() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let mut invocation = invocation(r#"{"path":"a.txt"}"#);
        invocation
            .metadata
            .insert("prompt_contract_epoch".to_string(), "3".to_string());
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
        let mut event = finished_event(&invocation, &result);
        event
            .metadata
            .insert("prompt_contract_epoch".to_string(), "3".to_string());
        store.append(event).expect("finished event should persist");

        assert!(completed_exact_tool_result(&store, &invocation)
            .expect("exact replay lookup should succeed")
            .is_some());
        invocation
            .metadata
            .insert("prompt_contract_epoch".to_string(), "4".to_string());
        assert!(completed_exact_tool_result(&store, &invocation)
            .expect("changed-lineage lookup should succeed")
            .is_none());
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
        result.model_observation = Some(
            ToolObservationV2::new(
                "filesystem.write",
                "Wrote the requested file.",
                "written",
                true,
                [("path".to_string(), "a.txt".to_string())]
                    .into_iter()
                    .collect(),
            )
            .with_next_action("Verify the file only if the task requires it."),
        );
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
        assert_eq!(replayed.model_observation, result.model_observation);
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
        let mut original = recovery_file_write_invocation(
            "call-1",
            "run-source",
            r#"{"path":"a.txt","content":"written"}"#,
        );
        original.metadata.insert(
            "logical_agent_run_id".to_string(),
            "logical-run".to_string(),
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

        let replayed =
            completed_recovery_effect_from_events(std::slice::from_ref(&event), &recovered, &root)
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
    fn recovery_effect_ledger_uses_the_source_patch_verifier() {
        let root = temporary_workspace("patch-effect-ledger");
        fs::write(root.join("a.txt"), "after").expect("applied patch state should exist");
        let input = r#"{"path":"a.txt","expected_base_sha256":"0000000000000000000000000000000000000000000000000000000000000000","replacement":"after","anchor":"before"}"#;
        let mut original = ToolInvocation {
            id: ToolCallId("call-patch-source".to_string()),
            task_id: TaskId("task-patch-ledger".to_string()),
            tool_name: "file.patch".to_string(),
            input_json: input.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: [
                ("project_id".to_string(), "project-1".to_string()),
                ("session_id".to_string(), "session-1".to_string()),
                ("agent_run_id".to_string(), "run-source".to_string()),
                (
                    agent_runtime::TOOL_RISK_METADATA_KEY.to_string(),
                    "writes_workspace".to_string(),
                ),
                (
                    agent_runtime::TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
                    "verifiable".to_string(),
                ),
                (
                    agent_runtime::TOOL_EFFECT_VERIFIER_METADATA_KEY.to_string(),
                    format!("workspace_file_sha256_v1:{:x}", Sha256::digest(b"after")),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let mut result = ToolResult::text(
            original.id.clone(),
            ToolOutcomeStatus::Succeeded,
            "patched",
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
        let mut store = SqliteStore::in_memory().expect("store should open");
        store
            .append(event)
            .expect("source finished event should persist");

        original.id = ToolCallId("call-patch-recovered".to_string());
        original
            .metadata
            .insert("agent_run_id".to_string(), "run-new".to_string());
        original
            .metadata
            .insert("source_agent_run_id".to_string(), "run-source".to_string());
        original
            .metadata
            .insert("recovery_resume_key".to_string(), "resume-1".to_string());
        original.metadata.insert(
            agent_runtime::TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
            "non_idempotent".to_string(),
        );
        original
            .metadata
            .remove(agent_runtime::TOOL_EFFECT_VERIFIER_METADATA_KEY);

        let recovered = completed_tool_result(&store, &original, &root)
            .expect("effect-ledger lookup should succeed")
            .expect("source after hash should verify the applied patch");
        assert_eq!(recovered.status, ToolOutcomeStatus::Succeeded);
        assert_eq!(
            recovered
                .metadata
                .get("effect_ledger_replay")
                .map(String::as_str),
            Some("true")
        );
        fs::remove_dir_all(root).expect("temporary workspace should be removed");
    }

    #[test]
    fn deferred_file_write_verification_reads_the_target_arguments() {
        let root = temporary_workspace("meta-effect-ledger");
        fs::write(root.join("a.txt"), "written").expect("effect should exist");
        let invocation = recovery_meta_file_write_invocation(
            r#"{"name":"file.write","arguments":{"path":"a.txt","content":"written"}}"#,
        );

        assert!(deterministic_effect_is_still_applied(&invocation, &root));

        let wrong_target = recovery_meta_file_write_invocation(
            r#"{"name":"shell.run","arguments":{"path":"a.txt","content":"written"}}"#,
        );
        assert!(!deterministic_effect_is_still_applied(&wrong_target, &root));

        let after_sha256 = format!("{:x}", Sha256::digest(b"written"));
        let mut deferred_patch = recovery_meta_file_write_invocation(
            r#"{"name":"file.patch","arguments":{"path":"a.txt","expected_base_sha256":"ignored-by-recovery","replacement":"written","anchor":"old"}}"#,
        );
        deferred_patch.metadata.insert(
            agent_runtime::TOOL_EFFECT_VERIFIER_METADATA_KEY.to_string(),
            format!("workspace_file_sha256_v1:{after_sha256}"),
        );
        assert!(deterministic_effect_is_still_applied(
            &deferred_patch,
            &root
        ));

        deferred_patch.input_json =
            r#"{"name":"shell.run","arguments":{"path":"a.txt"}}"#.to_string();
        assert!(!deterministic_effect_is_still_applied(
            &deferred_patch,
            &root
        ));
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
        recovered.metadata.insert(
            "logical_agent_run_id".to_string(),
            "logical-run".to_string(),
        );
        assert!(!agent_runtime::supports_recovery_effect_replay(&recovered));
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

    #[test]
    fn interrupted_patch_uses_the_recorded_after_hash_contract() {
        let root = temporary_workspace("patch-interrupted");
        fs::write(root.join("a.txt"), "after").expect("applied patch state should exist");
        let input = r#"{"path":"a.txt","expected_base_sha256":"0000000000000000000000000000000000000000000000000000000000000000","replacement":"after","anchor":"before"}"#;
        let mut invocation = ToolInvocation {
            id: ToolCallId("call-patch".to_string()),
            task_id: TaskId("task-patch".to_string()),
            tool_name: "file.patch".to_string(),
            input_json: input.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: [
                ("project_id".to_string(), "project-1".to_string()),
                ("session_id".to_string(), "session-1".to_string()),
                ("agent_run_id".to_string(), "run-1".to_string()),
                (
                    agent_runtime::TOOL_RISK_METADATA_KEY.to_string(),
                    "writes_workspace".to_string(),
                ),
                (
                    agent_runtime::TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
                    "non_idempotent".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let mut recorded = invocation.clone();
        recorded.metadata.insert(
            agent_runtime::TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
            "verifiable".to_string(),
        );
        recorded.metadata.insert(
            agent_runtime::TOOL_EFFECT_VERIFIER_METADATA_KEY.to_string(),
            format!("workspace_file_sha256_v1:{:x}", Sha256::digest(b"after")),
        );
        let mut store = SqliteStore::in_memory().expect("store should open");
        store
            .append(started_event(&recorded))
            .expect("started event should persist");

        let recovered = completed_tool_result(&store, &invocation, &root)
            .expect("recovery lookup should succeed")
            .expect("recorded after hash should verify the applied patch");
        assert_eq!(recovered.status, ToolOutcomeStatus::Succeeded);
        assert_eq!(
            recovered
                .metadata
                .get("effect_replay_mode")
                .map(String::as_str),
            Some("verified_interrupted_effect")
        );

        fs::write(root.join("a.txt"), "different").expect("state should change");
        invocation.metadata.insert(
            agent_runtime::TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
            "verifiable".to_string(),
        );
        let blocked = completed_tool_result(&store, &invocation, &root)
            .expect("recovery lookup should succeed")
            .expect("mismatched state should fail closed");
        assert_eq!(blocked.status, ToolOutcomeStatus::Failed);
        assert_eq!(
            blocked
                .failure
                .as_ref()
                .map(|failure| failure.code.as_str()),
            Some("tool_effect_outcome_unknown")
        );
        fs::remove_dir_all(root).expect("temporary workspace should be removed");
    }

    #[test]
    fn interrupted_external_effect_is_not_implicitly_repeated() {
        let mut invocation = invocation(r#"{"command":"send"}"#);
        invocation.tool_name = "browser.click".to_string();
        invocation.metadata.insert(
            agent_runtime::TOOL_RISK_METADATA_KEY.to_string(),
            "uses_network".to_string(),
        );

        assert!(
            interrupted_tool_call_started_event(&[started_event(&invocation)], &invocation)
                .is_some()
        );
        let result = unknown_interrupted_effect_result(&invocation);
        assert_eq!(result.status, ToolOutcomeStatus::Failed);
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.code.as_str()),
            Some("tool_effect_outcome_unknown")
        );
        assert_eq!(
            result
                .metadata
                .get("effect_recovery_mode")
                .map(String::as_str),
            Some("blocked_unknown_outcome")
        );
    }

    #[test]
    fn interrupted_read_only_call_remains_safe_to_retry() {
        let mut invocation = invocation(r#"{"path":"README.md"}"#);
        invocation.tool_name = "file.read".to_string();
        invocation.metadata.insert(
            agent_runtime::TOOL_RISK_METADATA_KEY.to_string(),
            "read_only".to_string(),
        );

        assert_eq!(
            tool_effect_recovery_policy(&invocation),
            ToolEffectRecoveryPolicy::SafeToRetry
        );
        assert!(
            interrupted_tool_call_started_event(&[started_event(&invocation)], &invocation)
                .is_some()
        );
    }
}
