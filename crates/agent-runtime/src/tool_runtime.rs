use agent_core::{
    Metadata, ToolArtifact, ToolFailure, ToolInvocation, ToolObservationV2, ToolOutcomeStatus,
    ToolResult, ToolRisk, ToolSpec, TOOL_OBSERVATION_V2_SCHEMA,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const TOOL_RESULT_SCHEMA: &str = "cindx.tool-result.v1";
pub const EFFECT_LEDGER_SCHEMA: &str = "cindx.effect-ledger.v1";
pub const TOOL_RISK_METADATA_KEY: &str = "tool_risk";
pub const TOOL_EFFECT_SEMANTICS_METADATA_KEY: &str = "tool_effect_semantics";
pub const TOOL_EFFECT_VERIFIER_METADATA_KEY: &str = "tool_effect_verifier";
pub const TOOL_MODEL_OBSERVATION_METADATA_KEY: &str = "model_observation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffectRecoveryPolicy {
    SafeToRetry,
    VerifyBeforeRetry,
    NeverRetryUnknown,
}

pub fn tool_risk_label(risk: &ToolRisk) -> &'static str {
    match risk {
        ToolRisk::ReadOnly => "read_only",
        ToolRisk::WritesWorkspace => "writes_workspace",
        ToolRisk::ExecutesProcess => "executes_process",
        ToolRisk::UsesNetwork => "uses_network",
        ToolRisk::SensitiveContext => "sensitive_context",
        ToolRisk::Destructive => "destructive",
    }
}

pub fn apply_tool_spec_runtime_metadata(invocation: &mut ToolInvocation, spec: &ToolSpec) {
    invocation.metadata.insert(
        TOOL_RISK_METADATA_KEY.to_string(),
        tool_risk_label(&spec.risk).to_string(),
    );
    invocation.metadata.insert(
        TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
        spec.effect_semantics.label().to_string(),
    );
    match spec.effect_semantics.verifier() {
        Some(verifier) => {
            invocation.metadata.insert(
                TOOL_EFFECT_VERIFIER_METADATA_KEY.to_string(),
                verifier.to_string(),
            );
        }
        None => {
            invocation
                .metadata
                .remove(TOOL_EFFECT_VERIFIER_METADATA_KEY);
        }
    }
}

pub fn tool_effect_recovery_policy(invocation: &ToolInvocation) -> ToolEffectRecoveryPolicy {
    match invocation
        .metadata
        .get(TOOL_EFFECT_SEMANTICS_METADATA_KEY)
        .map(String::as_str)
    {
        Some("read_only" | "idempotent") => ToolEffectRecoveryPolicy::SafeToRetry,
        Some("verifiable")
            if invocation
                .metadata
                .get(TOOL_EFFECT_VERIFIER_METADATA_KEY)
                .is_some_and(|verifier| !verifier.trim().is_empty()) =>
        {
            ToolEffectRecoveryPolicy::VerifyBeforeRetry
        }
        Some("verifiable" | "non_idempotent") => ToolEffectRecoveryPolicy::NeverRetryUnknown,
        _ => match invocation
            .metadata
            .get(TOOL_RISK_METADATA_KEY)
            .map(String::as_str)
        {
            Some("read_only") => ToolEffectRecoveryPolicy::SafeToRetry,
            Some("writes_workspace") if invocation.tool_name == "file.write" => {
                ToolEffectRecoveryPolicy::VerifyBeforeRetry
            }
            _ => ToolEffectRecoveryPolicy::NeverRetryUnknown,
        },
    }
}

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
    for key in [
        TOOL_RISK_METADATA_KEY,
        TOOL_EFFECT_SEMANTICS_METADATA_KEY,
        TOOL_EFFECT_VERIFIER_METADATA_KEY,
    ] {
        if let Some(value) = invocation.metadata.get(key) {
            metadata.insert(key.to_string(), value.clone());
        }
    }
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
    if let Some(observation) = &result.model_observation {
        let encoded = serde_json::json!({
            "schema": observation.schema,
            "tool_name": observation.tool_name,
            "summary": observation.summary,
            "evidence": observation.evidence,
            "evidence_complete": observation.evidence_complete,
            "facts": observation.facts,
            "next_action": observation.next_action,
        })
        .to_string();
        result
            .metadata
            .insert(TOOL_MODEL_OBSERVATION_METADATA_KEY.to_string(), encoded);
    }
}

pub fn decode_persisted_tool_model_observation(metadata: &Metadata) -> Option<ToolObservationV2> {
    let value = metadata
        .get(TOOL_MODEL_OBSERVATION_METADATA_KEY)
        .and_then(|encoded| serde_json::from_str::<serde_json::Value>(encoded).ok())?;
    let facts = value
        .get("facts")?
        .as_object()?
        .iter()
        .map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
        .collect::<Option<Metadata>>()?;
    let observation = ToolObservationV2 {
        schema: value.get("schema")?.as_str()?.to_string(),
        tool_name: value.get("tool_name")?.as_str()?.to_string(),
        summary: value.get("summary")?.as_str()?.to_string(),
        evidence: value.get("evidence")?.as_str()?.to_string(),
        evidence_complete: value.get("evidence_complete")?.as_bool()?,
        facts,
        next_action: match value.get("next_action") {
            Some(serde_json::Value::String(value)) => Some(value.clone()),
            Some(serde_json::Value::Null) | None => None,
            Some(_) => return None,
        },
    };
    (observation.schema == TOOL_OBSERVATION_V2_SCHEMA).then_some(observation)
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
    tool_effect_recovery_policy(invocation) == ToolEffectRecoveryPolicy::VerifyBeforeRetry
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
    use agent_core::{TaskId, ToolCallId, ToolEffectSemantics};

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
    fn tool_spec_metadata_is_authoritative_and_clears_stale_verifiers() {
        let mut write = invocation(r#"{"path":"a.txt","content":"ok"}"#);
        write.metadata.insert(
            TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
            "non_idempotent".to_string(),
        );
        let write_spec = ToolSpec::builtin(
            "file.write",
            "file",
            "write",
            ToolRisk::WritesWorkspace,
            "{}",
        )
        .with_effect_semantics(ToolEffectSemantics::Verifiable {
            verifier: "workspace_file_content_v1".to_string(),
        });

        apply_tool_spec_runtime_metadata(&mut write, &write_spec);

        assert_eq!(
            write
                .metadata
                .get(TOOL_EFFECT_SEMANTICS_METADATA_KEY)
                .map(String::as_str),
            Some("verifiable")
        );
        assert_eq!(
            write
                .metadata
                .get(TOOL_EFFECT_VERIFIER_METADATA_KEY)
                .map(String::as_str),
            Some("workspace_file_content_v1")
        );

        let read_spec = ToolSpec::builtin("file.read", "file", "read", ToolRisk::ReadOnly, "{}");
        apply_tool_spec_runtime_metadata(&mut write, &read_spec);
        assert_eq!(
            write
                .metadata
                .get(TOOL_EFFECT_SEMANTICS_METADATA_KEY)
                .map(String::as_str),
            Some("read_only")
        );
        assert!(!write
            .metadata
            .contains_key(TOOL_EFFECT_VERIFIER_METADATA_KEY));
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

    #[test]
    fn effect_recovery_policy_is_explicit_and_conservative() {
        let mut read = invocation("{}");
        read.tool_name = "file.read".to_string();
        read.metadata.insert(
            TOOL_RISK_METADATA_KEY.to_string(),
            tool_risk_label(&ToolRisk::ReadOnly).to_string(),
        );
        assert_eq!(
            tool_effect_recovery_policy(&read),
            ToolEffectRecoveryPolicy::SafeToRetry
        );

        let mut idempotent = invocation("{}");
        idempotent.tool_name = "browser.close".to_string();
        idempotent.metadata.insert(
            TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
            "idempotent".to_string(),
        );
        assert_eq!(
            tool_effect_recovery_policy(&idempotent),
            ToolEffectRecoveryPolicy::SafeToRetry
        );

        let mut write = invocation(r#"{"path":"a.txt","content":"ok"}"#);
        write.metadata.insert(
            TOOL_RISK_METADATA_KEY.to_string(),
            tool_risk_label(&ToolRisk::WritesWorkspace).to_string(),
        );
        assert_eq!(
            tool_effect_recovery_policy(&write),
            ToolEffectRecoveryPolicy::VerifyBeforeRetry
        );
        write.metadata.insert(
            TOOL_EFFECT_SEMANTICS_METADATA_KEY.to_string(),
            "verifiable".to_string(),
        );
        assert_eq!(
            tool_effect_recovery_policy(&write),
            ToolEffectRecoveryPolicy::NeverRetryUnknown
        );
        write.metadata.insert(
            TOOL_EFFECT_VERIFIER_METADATA_KEY.to_string(),
            "workspace_file_content_v1".to_string(),
        );
        assert_eq!(
            tool_effect_recovery_policy(&write),
            ToolEffectRecoveryPolicy::VerifyBeforeRetry
        );

        let mut shell = invocation(r#"{"command":"echo ok"}"#);
        shell.tool_name = "shell.run".to_string();
        shell.metadata.insert(
            TOOL_RISK_METADATA_KEY.to_string(),
            tool_risk_label(&ToolRisk::ExecutesProcess).to_string(),
        );
        assert_eq!(
            tool_effect_recovery_policy(&shell),
            ToolEffectRecoveryPolicy::NeverRetryUnknown
        );
    }
}
