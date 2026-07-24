use agent_core::{
    Metadata, ToolArtifact, ToolFailure, ToolInvocation, ToolOutcomeStatus, ToolResult,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const TOOL_RESULT_SCHEMA: &str = "cindx.tool-result.v1";
pub const EFFECT_LEDGER_SCHEMA: &str = "cindx.effect-ledger.v1";

const EXECUTION_SCOPE_KEYS: [&str; 4] = [
    "project_id",
    "session_id",
    "agent_run_id",
    "collaboration_id",
];
const EVENT_CONTEXT_KEYS: [&str; 5] = [
    "project_id",
    "session_id",
    "agent_run_id",
    "collaboration_id",
    "prompt_profile",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedToolArtifact {
    path: String,
    mime_type: Option<String>,
    title: Option<String>,
}

pub fn tool_input_fingerprint(tool_name: &str, input_json: &str) -> String {
    let canonical_input = serde_json::from_str::<serde_json::Value>(input_json)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| input_json.trim().to_string());
    let mut digest = Sha256::new();
    digest.update(tool_name.as_bytes());
    digest.update(b"\n");
    digest.update(canonical_input.as_bytes());
    format!("{:x}", digest.finalize())
}

pub fn tool_invocation_event_metadata(invocation: &ToolInvocation) -> Metadata {
    let input_fingerprint = tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
    let mut metadata = [
        ("tool_call_id".to_string(), invocation.id.0.clone()),
        ("tool".to_string(), invocation.tool_name.clone()),
        ("input_fingerprint".to_string(), input_fingerprint.clone()),
        ("effect_fingerprint".to_string(), input_fingerprint),
        (
            "effect_ledger_schema".to_string(),
            EFFECT_LEDGER_SCHEMA.to_string(),
        ),
        (
            "input_length".to_string(),
            invocation.input_json.len().to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    metadata.extend(tool_invocation_context(invocation));
    metadata
}

pub fn tool_invocation_context(invocation: &ToolInvocation) -> Metadata {
    EVENT_CONTEXT_KEYS
        .iter()
        .filter_map(|key| {
            invocation
                .metadata
                .get(*key)
                .map(|value| ((*key).to_string(), value.clone()))
        })
        .collect()
}

pub fn finalize_tool_result(
    result: &mut ToolResult,
    invocation_id: &agent_core::ToolCallId,
    input_fingerprint: &str,
    elapsed: Duration,
) {
    if result.invocation_id != *invocation_id {
        result.metadata.insert(
            "reported_invocation_id".to_string(),
            result.invocation_id.0.clone(),
        );
        result.invocation_id = invocation_id.clone();
    }
    if matches!(result.status, ToolOutcomeStatus::Failed) && result.failure.is_none() {
        result.failure = Some(ToolFailure {
            code: "tool_execution_failed".to_string(),
            message: result.output.clone(),
            retryable: false,
        });
    }

    result.metadata.insert(
        "tool_result_schema".to_string(),
        TOOL_RESULT_SCHEMA.to_string(),
    );
    result.metadata.insert(
        "effect_ledger_schema".to_string(),
        EFFECT_LEDGER_SCHEMA.to_string(),
    );
    result.metadata.insert(
        "input_fingerprint".to_string(),
        input_fingerprint.to_string(),
    );
    result.metadata.insert(
        "effect_fingerprint".to_string(),
        input_fingerprint.to_string(),
    );
    result.metadata.insert(
        "latency_ms".to_string(),
        elapsed.as_millis().min(u128::from(u64::MAX)).to_string(),
    );
    if let Some(failure) = &result.failure {
        result
            .metadata
            .insert("failure_code".to_string(), failure.code.clone());
        result.metadata.insert(
            "failure_retryable".to_string(),
            failure.retryable.to_string(),
        );
    }
    if !result.artifacts.is_empty() {
        let artifacts = result
            .artifacts
            .iter()
            .map(|artifact| PersistedToolArtifact {
                path: artifact.path.clone(),
                mime_type: artifact.mime_type.clone(),
                title: artifact.title.clone(),
            })
            .collect::<Vec<_>>();
        if let Ok(encoded) = serde_json::to_string(&artifacts) {
            result
                .metadata
                .insert("artifacts_json".to_string(), encoded);
        }
    }
}

pub fn decode_persisted_tool_artifacts(metadata: &Metadata) -> Vec<ToolArtifact> {
    metadata
        .get("artifacts_json")
        .and_then(|encoded| serde_json::from_str::<Vec<PersistedToolArtifact>>(encoded).ok())
        .map(|artifacts| {
            artifacts
                .into_iter()
                .map(|artifact| ToolArtifact {
                    path: artifact.path,
                    mime_type: artifact.mime_type,
                    title: artifact.title,
                })
                .collect()
        })
        .unwrap_or_else(|| {
            metadata
                .get("artifact_path")
                .map(|path| {
                    vec![ToolArtifact {
                        path: path.clone(),
                        mime_type: None,
                        title: None,
                    }]
                })
                .unwrap_or_default()
        })
}

pub fn tool_execution_scope_matches(
    event_metadata: &Metadata,
    invocation: &ToolInvocation,
) -> bool {
    EXECUTION_SCOPE_KEYS.iter().all(|key| {
        let Some(expected) = invocation.metadata.get(*key) else {
            return true;
        };
        if event_metadata.get(*key) == Some(expected) {
            return true;
        }
        *key == "agent_run_id"
            && invocation
                .metadata
                .get("source_agent_run_id")
                .is_some_and(|source| event_metadata.get(*key) == Some(source))
    })
}

pub fn supports_recovery_effect_replay(invocation: &ToolInvocation) -> bool {
    invocation.tool_name == "file.write"
        && invocation
            .metadata
            .get("source_agent_run_id")
            .is_some_and(|value| !value.trim().is_empty())
        && invocation
            .metadata
            .get("recovery_resume_key")
            .is_some_and(|value| !value.trim().is_empty())
}

pub fn recovery_source_scope_matches(
    event_metadata: &Metadata,
    invocation: &ToolInvocation,
) -> bool {
    let Some(source_run_id) = invocation.metadata.get("source_agent_run_id") else {
        return false;
    };
    if event_metadata.get("agent_run_id") != Some(source_run_id) {
        return false;
    }
    ["project_id", "session_id", "collaboration_id"]
        .iter()
        .all(|key| {
            invocation
                .metadata
                .get(*key)
                .is_none_or(|expected| event_metadata.get(*key) == Some(expected))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{TaskId, ToolCallId};

    fn invocation(input_json: &str) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId("call-1".to_string()),
            task_id: TaskId("task-1".to_string()),
            tool_name: "file.write".to_string(),
            input_json: input_json.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: [("session_id".to_string(), "session-1".to_string())]
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
    fn execution_scope_accepts_only_matching_or_recovery_source_runs() {
        let mut invocation = invocation("{}");
        invocation
            .metadata
            .insert("agent_run_id".to_string(), "run-new".to_string());
        let event = [
            ("session_id".to_string(), "session-1".to_string()),
            ("agent_run_id".to_string(), "run-source".to_string()),
        ]
        .into_iter()
        .collect();
        assert!(!tool_execution_scope_matches(&event, &invocation));
        invocation
            .metadata
            .insert("source_agent_run_id".to_string(), "run-source".to_string());
        assert!(tool_execution_scope_matches(&event, &invocation));
    }
}
