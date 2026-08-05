use agent_core::{
    Metadata, ToolArtifact, ToolFailure, ToolInvocation, ToolObservationV2, ToolOutcomeStatus,
    ToolResult, ToolRisk, ToolSpec, TOOL_OBSERVATION_V2_SCHEMA,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::time::Duration;

pub const TOOL_RESULT_SCHEMA: &str = "cindx.tool-result.v1";
pub const EFFECT_LEDGER_SCHEMA: &str = "cindx.effect-ledger.v1";
pub const TOOL_RISK_METADATA_KEY: &str = "tool_risk";
pub const TOOL_EFFECT_SEMANTICS_METADATA_KEY: &str = "tool_effect_semantics";
pub const TOOL_EFFECT_VERIFIER_METADATA_KEY: &str = "tool_effect_verifier";
pub const TOOL_MODEL_OBSERVATION_METADATA_KEY: &str = "model_observation";
pub const TOOL_EFFECT_WITNESS_METADATA_KEY: &str = "tool_effect_witness";
pub const TOOL_EFFECT_WITNESS_SCHEMA: &str = "cindx.tool-effect-witness.v1";
pub const MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES: usize = 4_096;

const MAX_EFFECT_WITNESS_TARGETS: usize = 8;
const MAX_EFFECT_WITNESS_PATH_COMPONENTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedToolEffectKind {
    WorkspaceMutation,
    WorkspaceObservation,
    ProcessVerification,
    BrowserAction,
    BrowserObservation,
    ComputerAction,
    ComputerObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedToolEffectWitness {
    schema: String,
    tool_name: String,
    input_fingerprint: String,
    kind: PersistedToolEffectKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    target_tokens: Vec<String>,
}

impl PersistedToolEffectWitness {
    pub fn capture(
        tool_name: &str,
        input_json: &str,
        risk: Option<&ToolRisk>,
        lineage_scope: &str,
    ) -> Option<Self> {
        let kind = match tool_name {
            "browser.open" | "browser.click" | "browser.type" | "browser.scroll"
            | "browser.select_tab" => PersistedToolEffectKind::BrowserAction,
            "browser.extract_text" | "browser.capture" | "browser.tabs" => {
                PersistedToolEffectKind::BrowserObservation
            }
            "computer.click" | "computer.type" | "computer.key" | "computer.scroll" => {
                PersistedToolEffectKind::ComputerAction
            }
            "computer.screenshot" => PersistedToolEffectKind::ComputerObservation,
            _ => match risk {
                Some(ToolRisk::WritesWorkspace | ToolRisk::Destructive) => {
                    PersistedToolEffectKind::WorkspaceMutation
                }
                Some(ToolRisk::ReadOnly) if !structured_effect_targets(input_json).is_empty() => {
                    PersistedToolEffectKind::WorkspaceObservation
                }
                Some(ToolRisk::ExecutesProcess)
                    if process_input_looks_like_verification(input_json) =>
                {
                    PersistedToolEffectKind::ProcessVerification
                }
                _ => return None,
            },
        };
        let target_tokens = if matches!(
            kind,
            PersistedToolEffectKind::WorkspaceMutation
                | PersistedToolEffectKind::WorkspaceObservation
        ) {
            redacted_effect_targets(input_json, lineage_scope)
        } else {
            Vec::new()
        };
        let witness = Self {
            schema: TOOL_EFFECT_WITNESS_SCHEMA.to_string(),
            tool_name: tool_name.to_string(),
            input_fingerprint: tool_input_fingerprint(tool_name, input_json),
            kind,
            target_tokens,
        };
        witness.is_valid().then_some(witness)
    }

    pub fn encode(&self) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        let encoded = serde_json::to_string(self).ok()?;
        (encoded.len() <= MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES).then_some(encoded)
    }

    pub fn decode(encoded: &str) -> Option<Self> {
        if encoded.len() > MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES {
            return None;
        }
        let witness = serde_json::from_str::<Self>(encoded).ok()?;
        witness.is_valid().then_some(witness)
    }

    pub fn replay_for(
        &self,
        tool_name: &str,
        input_fingerprint: &str,
        registered_risk: Option<&ToolRisk>,
    ) -> Option<String> {
        if !self.is_valid()
            || self.tool_name != tool_name
            || self.input_fingerprint != input_fingerprint
            || !self.matches_tool_and_risk(tool_name, registered_risk)
        {
            return None;
        }
        let input = match self.kind {
            PersistedToolEffectKind::WorkspaceMutation
            | PersistedToolEffectKind::WorkspaceObservation => {
                serde_json::json!({ "path": self.target_tokens }).to_string()
            }
            PersistedToolEffectKind::ProcessVerification => {
                serde_json::json!({ "command": "verify" }).to_string()
            }
            PersistedToolEffectKind::BrowserAction
            | PersistedToolEffectKind::BrowserObservation
            | PersistedToolEffectKind::ComputerAction
            | PersistedToolEffectKind::ComputerObservation => "{}".to_string(),
        };
        Some(input)
    }

    fn matches_tool_and_risk(&self, tool_name: &str, risk: Option<&ToolRisk>) -> bool {
        match self.kind {
            PersistedToolEffectKind::WorkspaceMutation => {
                matches!(
                    risk,
                    Some(ToolRisk::WritesWorkspace | ToolRisk::Destructive)
                )
            }
            PersistedToolEffectKind::WorkspaceObservation => {
                matches!(risk, Some(ToolRisk::ReadOnly))
            }
            PersistedToolEffectKind::ProcessVerification => {
                matches!(risk, Some(ToolRisk::ExecutesProcess))
            }
            PersistedToolEffectKind::BrowserAction => matches!(
                (tool_name, risk),
                (
                    "browser.open" | "browser.click" | "browser.scroll" | "browser.select_tab",
                    Some(ToolRisk::UsesNetwork)
                ) | ("browser.type", Some(ToolRisk::SensitiveContext))
            ),
            PersistedToolEffectKind::BrowserObservation => matches!(
                (tool_name, risk),
                (
                    "browser.extract_text" | "browser.capture" | "browser.tabs",
                    Some(ToolRisk::UsesNetwork)
                )
            ),
            PersistedToolEffectKind::ComputerAction => matches!(
                (tool_name, risk),
                (
                    "computer.click" | "computer.type" | "computer.scroll",
                    Some(ToolRisk::SensitiveContext)
                ) | ("computer.key", Some(ToolRisk::Destructive))
            ),
            PersistedToolEffectKind::ComputerObservation => matches!(
                (tool_name, risk),
                ("computer.screenshot", Some(ToolRisk::SensitiveContext))
            ),
        }
    }

    fn is_valid(&self) -> bool {
        if self.schema != TOOL_EFFECT_WITNESS_SCHEMA
            || self.tool_name.trim().is_empty()
            || self.tool_name.len() > 128
            || self.input_fingerprint.len() != 64
            || !self
                .input_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.target_tokens.len() > MAX_EFFECT_WITNESS_TARGETS
            || self.target_tokens.iter().any(|target| {
                target.len() > 320
                    || !target.starts_with("redacted/")
                    || !target["redacted/".len()..]
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() || byte == b'/')
            })
        {
            return false;
        }
        match self.kind {
            PersistedToolEffectKind::WorkspaceMutation => true,
            PersistedToolEffectKind::WorkspaceObservation => !self.target_tokens.is_empty(),
            _ => self.target_tokens.is_empty(),
        }
    }
}

fn redacted_effect_targets(input_json: &str, lineage_scope: &str) -> Vec<String> {
    structured_effect_targets(input_json)
        .into_iter()
        .filter_map(|target| redacted_effect_target(&target, lineage_scope))
        .take(MAX_EFFECT_WITNESS_TARGETS)
        .collect()
}

fn structured_effect_targets(input_json: &str) -> BTreeSet<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(input_json) else {
        return BTreeSet::new();
    };
    let mut targets = BTreeSet::new();
    collect_effect_targets(&value, None, &mut targets);
    targets
}

fn collect_effect_targets(
    value: &serde_json::Value,
    key: Option<&str>,
    targets: &mut BTreeSet<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                collect_effect_targets(value, Some(key), targets);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_effect_targets(value, key, targets);
            }
        }
        serde_json::Value::String(value)
            if key.is_some_and(|key| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "path" | "file" | "file_path" | "output" | "output_path" | "directory"
                )
            }) =>
        {
            let normalized = value.trim().trim_end_matches(['/', '\\']).to_string();
            if !normalized.is_empty() {
                targets.insert(normalized);
            }
        }
        _ => {}
    }
}

fn redacted_effect_target(target: &str, lineage_scope: &str) -> Option<String> {
    let normalized = target.replace('\\', "/");
    let absolute = normalized.starts_with('/')
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|byte| *byte == b':');
    let mut components = Vec::<&str>::new();
    for component in normalized.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    if components.is_empty() {
        return None;
    }

    let root = if absolute { "absolute" } else { "relative" };
    let mut cumulative = String::new();
    let mut tokens = Vec::new();
    let prefix_count = components.len().min(MAX_EFFECT_WITNESS_PATH_COMPONENTS);
    for component in components.iter().take(prefix_count) {
        if !cumulative.is_empty() {
            cumulative.push('/');
        }
        cumulative.push_str(component);
        tokens.push(effect_target_digest(lineage_scope, root, &cumulative));
    }
    if components.len() > MAX_EFFECT_WITNESS_PATH_COMPONENTS {
        let full_target = components.join("/");
        let full_digest = effect_target_digest(lineage_scope, root, &full_target);
        if tokens.last() != Some(&full_digest) {
            tokens.pop();
            tokens.push(full_digest);
        }
    }
    Some(format!("redacted/{}", tokens.join("/")))
}

fn effect_target_digest(lineage_scope: &str, root: &str, target: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(TOOL_EFFECT_WITNESS_SCHEMA.as_bytes());
    digest.update(b"\n");
    digest.update(lineage_scope.as_bytes());
    digest.update(b"\n");
    digest.update(root.as_bytes());
    digest.update(b"\n");
    digest.update(target.as_bytes());
    format!("{:x}", digest.finalize())[..32].to_string()
}

fn process_input_looks_like_verification(input_json: &str) -> bool {
    let normalized = input_json.to_ascii_lowercase();
    [
        " test",
        "test ",
        "check",
        "build",
        "lint",
        "verify",
        "pytest",
        "vitest",
        "jest",
        "cargo test",
        "cargo check",
        "swift test",
        "go test",
        "git diff",
        "git status",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

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
    fn persisted_effect_witness_is_bounded_redacted_and_target_stable() {
        let write_input =
            r#"{"path":"private/super-secret/goal.md","content":"never-persist-this-secret"}"#;
        let read_input = r#"{"path":"private/super-secret/goal.md"}"#;
        let write = PersistedToolEffectWitness::capture(
            "file.write",
            write_input,
            Some(&ToolRisk::WritesWorkspace),
            "run-a:4",
        )
        .expect("workspace mutation should have a durable witness");
        let read = PersistedToolEffectWitness::capture(
            "file.read",
            read_input,
            Some(&ToolRisk::ReadOnly),
            "run-a:4",
        )
        .expect("targeted read should have a durable witness");
        let encoded_write = write.encode().expect("write witness encodes");
        let encoded_read = read.encode().expect("read witness encodes");

        assert!(encoded_write.len() <= MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES);
        assert!(encoded_read.len() <= MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES);
        for secret in [
            "private",
            "super-secret",
            "goal.md",
            "never-persist-this-secret",
        ] {
            assert!(!encoded_write.contains(secret));
            assert!(!encoded_read.contains(secret));
        }
        assert_eq!(
            write.replay_for(
                "file.write",
                &tool_input_fingerprint("file.write", write_input),
                Some(&ToolRisk::WritesWorkspace),
            ),
            read.replay_for(
                "file.read",
                &tool_input_fingerprint("file.read", read_input),
                Some(&ToolRisk::ReadOnly),
            ),
            "same target in one run should replay to the same pseudonymous path"
        );
        assert_eq!(
            PersistedToolEffectWitness::decode(&encoded_write),
            Some(write)
        );
    }

    #[test]
    fn persisted_effect_witness_falls_back_safely_across_versions() {
        let legacy_process = r#"{"kind":"process_verification"}"#;
        assert!(
            PersistedToolEffectWitness::decode(legacy_process).is_none(),
            "an unknown object without the exact schema and invocation binding fails closed"
        );
        assert!(PersistedToolEffectWitness::decode(
            &"x".repeat(MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES + 1)
        )
        .is_none());

        let browser = PersistedToolEffectWitness::capture(
            "browser.click",
            r##"{"selector":"#secret-button"}"##,
            Some(&ToolRisk::UsesNetwork),
            "run-a:4",
        )
        .expect("interaction action should have a typed witness");
        let browser_fingerprint =
            tool_input_fingerprint("browser.click", r##"{"selector":"#secret-button"}"##);
        assert!(browser
            .replay_for(
                "browser.click",
                &browser_fingerprint,
                Some(&ToolRisk::UsesNetwork),
            )
            .is_some());
        assert!(browser
            .replay_for(
                "computer.click",
                &browser_fingerprint,
                Some(&ToolRisk::SensitiveContext),
            )
            .is_none());
        assert!(browser
            .replay_for(
                "browser.click",
                &tool_input_fingerprint("browser.click", r##"{"selector":"#other"}"##),
                Some(&ToolRisk::UsesNetwork),
            )
            .is_none());
        let encoded_browser = browser.encode().expect("browser witness encodes");
        assert!(!encoded_browser.contains("secret-button"));
        let mut mismatched =
            serde_json::from_str::<serde_json::Value>(&encoded_browser).expect("witness is JSON");
        mismatched["kind"] = serde_json::Value::String("workspace_mutation".to_string());
        let mismatched = PersistedToolEffectWitness::decode(&mismatched.to_string())
            .expect("shape remains decodable");
        assert!(mismatched
            .replay_for(
                "browser.click",
                &browser_fingerprint,
                Some(&ToolRisk::UsesNetwork),
            )
            .is_none());
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
