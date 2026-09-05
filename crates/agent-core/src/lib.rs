use std::collections::{BTreeMap, BTreeSet};

mod agent_policy;
mod direct_judge;
mod event_contract;
mod exec_policy;
pub mod execution_contract;
mod http_policy;
mod knowledge_plan;
mod learning_evidence;
mod model_attribution;
mod model_candidate;
mod model_contract;
mod orchestration_policy;
mod permission_policy;
mod routing_telemetry;
pub mod run_decision_enums;
mod run_identity;
mod run_requirements;
mod sandbox;

pub use agent_policy::AgentPolicy;
pub use direct_judge::{
    direct_judge_eligible, direct_judge_model, direct_judge_prompt, direct_judge_repair_directive,
    DirectJudgeClaim, DirectJudgeClaimStatus, DirectJudgeReceipt, DirectJudgeVerdict,
    DIRECT_JUDGE_MAX_CLAIMS, DIRECT_JUDGE_MAX_CLAIM_SUMMARY_BYTES, DIRECT_JUDGE_MAX_REPAIR_ROUNDS,
    DIRECT_JUDGE_RECEIPT_SCHEMA,
};
pub use event_contract::{
    decode_event_type, insert_event_type_v1, DecodedEventType, EventTypeBuildError, EventTypeV1,
    TypedEventRef, EVENT_TYPE_METADATA_KEY,
};
pub use exec_policy::{
    command_prefix_for_grant, is_dangerous_command, prefix_rule_matches, shell_command_tokens,
    ExecPrefixRule,
};
pub use execution_contract::{
    ConductorExecutionContract, ConductorFallbackPolicy, ConductorStopPolicy, MAX_PLANNING_STEPS,
};
pub use http_policy::{
    curl_policy_args, http_policy, ip_is_public, resolve_redirect_location,
    validate_public_http_url, HttpEgressProfile, HttpPolicy, HttpPolicyError, ProxyMode,
    PublicHttpTarget, HTTP_MCP_DEFAULT_TIMEOUT_MS, HTTP_MCP_RESPONSE_MAX_BYTES,
    HTTP_MODEL_RESPONSE_MAX_BYTES, HTTP_PROVIDER_MAX_REDIRECTS, HTTP_PUBLIC_FETCH_MAX_REDIRECTS,
    HTTP_SKILL_PACKAGE_MAX_BYTES, HTTP_SKILL_TIMEOUT_SECONDS, HTTP_STDERR_MAX_BYTES,
    HTTP_USER_AGENT, HTTP_WEB_MAX_TIMEOUT_SECONDS, HTTP_WEB_RESPONSE_MAX_BYTES,
    HTTP_WEB_TIMEOUT_SECONDS,
};
pub use knowledge_plan::{
    MemoryRecallPlan, MemoryRecallPolicy, WorkspaceRetrievalChannel, WorkspaceRetrievalPlan,
};
pub use learning_evidence::{
    IndependentQualitySource, LearningAttribution, LearningDisposition, LearningEvidenceSchema,
    LearningEvidenceV1, LearningTermination, LearningUsageCompleteness, LearningVerification,
    LEARNING_EVIDENCE_MAX_BYTES, LEARNING_EVIDENCE_SCHEMA_V1,
};
pub use model_attribution::{
    AgentActor, AgentEffectAuthority, AgentModelAttribution, AgentModelAttributionError,
    AgentModelProfile, AgentOutputTrust, AgentService, AgentStage, AGENT_ACTOR_METADATA_KEY,
    AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY, AGENT_ATTRIBUTION_LEGACY_ROLE_METADATA_KEY,
    AGENT_ATTRIBUTION_MODEL_METADATA_KEY, AGENT_EFFECT_AUTHORITY_METADATA_KEY,
    AGENT_MODEL_ATTRIBUTION_SCHEMA, AGENT_MODEL_ATTRIBUTION_SCHEMA_METADATA_KEY,
    AGENT_MODEL_PROFILE_METADATA_KEY, AGENT_OUTPUT_TRUST_METADATA_KEY, AGENT_SERVICE_METADATA_KEY,
    AGENT_STAGE_METADATA_KEY,
};
pub use model_candidate::{ModelCandidate, ModelCapabilitySource, TaskClass};
pub use model_contract::{
    classify_provider_failure, tool_function_name, ModelCallMode, ModelError, ModelRequest,
    ModelResponse, ModelResponseAssessment, ModelResponseDisposition, ModelResponseTermination,
    ModelToolCall, ProviderFailureClass, GENERATION_TEMPERATURE_KEY, REASONING_EFFORT_KEY,
    THINKING_BUDGET_KEY,
};
pub use orchestration_policy::{parse_policy, role_label, OrchestrationPolicy};
pub use permission_policy::{
    permission_can_allow_session, permission_capability_matches, permission_requires_exact_scope,
};
pub use routing_telemetry::{RoutingOutcome, RoutingTelemetry};
pub use run_decision_enums::AgentToolRequirement;
pub use run_identity::{
    agent_run_id, logical_agent_run_id, source_agent_run_id, AgentRunIdentity,
    AgentRunIdentityError, AgentRunLineage, AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY,
    AGENT_RUN_IDENTITY_V1_SCHEMA, AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY,
    SOURCE_AGENT_RUN_ID_METADATA_KEY,
};
pub use run_requirements::{
    AgentRiskLevel, AgentRouteRequirements, AGENT_RUN_DECISION_SCHEMA,
    MAX_RUN_DECISION_QUERY_CHARS, MAX_RUN_DECISION_RATIONALE_CHARS,
};
pub use sandbox::{
    confined_argv, sandbox_mode_from_metadata, sandboxed_argv, sbpl_escape, seatbelt_profile_args,
    SandboxMode, SANDBOX_MODE_METADATA_KEY,
};

pub type Metadata = BTreeMap<String, String>;

pub const TOOL_OBSERVATION_V2_SCHEMA: &str = "cindx.tool-observation.v2";
pub const LEARNING_EVIDENCE_METADATA_KEY: &str = "learning_evidence_v1";

pub fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(value))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolCallId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PermissionRequestId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    WaitingForPermission,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
    Reviewer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    TaskCreated,
    TaskStatusChanged,
    MessageAdded,
    ModelRequestStarted,
    ModelRequestFinished,
    ToolCallProposed,
    ToolCallStarted,
    ToolCallFinished,
    PermissionRequested,
    PermissionResolved,
    RetrievalPerformed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: EventId,
    pub task_id: TaskId,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub kind: EventKind,
    pub summary: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionRisk {
    Read,
    Write,
    Execute,
    Network,
    Sensitive,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    AllowForSession,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub id: PermissionRequestId,
    pub task_id: TaskId,
    pub risk: PermissionRisk,
    pub action: String,
    pub reason: String,
    pub scope: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionResolution {
    pub request_id: PermissionRequestId,
    pub decision: PermissionDecision,
    pub resolved_at_ms: u64,
    pub resolved_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRisk {
    ReadOnly,
    WritesWorkspace,
    ExecutesProcess,
    UsesNetwork,
    SensitiveContext,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    BuiltIn,
    Mcp { server_id: String },
    Skill { skill_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    Inline,
    Deferred,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEffectSemantics {
    ReadOnly,
    Idempotent,
    Verifiable { verifier: String },
    NonIdempotent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PostconditionVerifierKind {
    WorkspaceExactReadbackV1,
    WorkspaceQualityCheckV1,
}

impl PostconditionVerifierKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::WorkspaceExactReadbackV1 => "workspace_exact_readback_v1",
            Self::WorkspaceQualityCheckV1 => "workspace_quality_check_v1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPostconditionEvidence {
    pub kind: PostconditionVerifierKind,
    pub target_input_json: String,
}

impl ToolEffectSemantics {
    pub fn conservative_default(risk: &ToolRisk) -> Self {
        if matches!(risk, ToolRisk::ReadOnly) {
            Self::ReadOnly
        } else {
            Self::NonIdempotent
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Idempotent => "idempotent",
            Self::Verifiable { .. } => "verifiable",
            Self::NonIdempotent => "non_idempotent",
        }
    }

    pub fn verifier(&self) -> Option<&str> {
        match self {
            Self::Verifiable { verifier } => Some(verifier.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolExecutionConcurrency {
    #[default]
    Serialized,
    IndependentRead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub risk: ToolRisk,
    pub source: ToolSource,
    pub exposure: ToolExposure,
    pub input_schema_json: String,
    pub output_schema_json: Option<String>,
    pub effect_semantics: ToolEffectSemantics,
    pub execution_concurrency: ToolExecutionConcurrency,
    pub postcondition_verifiers: BTreeSet<PostconditionVerifierKind>,
}

impl ToolSpec {
    pub fn new(
        name: impl Into<String>,
        namespace: impl Into<String>,
        description: impl Into<String>,
        risk: ToolRisk,
        source: ToolSource,
        exposure: ToolExposure,
        input_schema_json: impl Into<String>,
    ) -> Self {
        let effect_semantics = ToolEffectSemantics::conservative_default(&risk);
        Self {
            name: name.into(),
            namespace: namespace.into(),
            description: description.into(),
            risk,
            source,
            exposure,
            input_schema_json: input_schema_json.into(),
            output_schema_json: None,
            effect_semantics,
            execution_concurrency: ToolExecutionConcurrency::default(),
            postcondition_verifiers: BTreeSet::new(),
        }
    }

    pub fn with_effect_semantics(mut self, effect_semantics: ToolEffectSemantics) -> Self {
        self.effect_semantics = effect_semantics;
        self
    }

    pub fn with_execution_concurrency(
        mut self,
        execution_concurrency: ToolExecutionConcurrency,
    ) -> Self {
        self.execution_concurrency = execution_concurrency;
        self
    }

    pub fn with_postcondition_verifier(mut self, verifier: PostconditionVerifierKind) -> Self {
        self.postcondition_verifiers.insert(verifier);
        self
    }

    pub fn with_output_schema(mut self, output_schema_json: impl Into<String>) -> Self {
        self.output_schema_json = Some(output_schema_json.into());
        self
    }

    pub fn builtin(
        name: impl Into<String>,
        namespace: impl Into<String>,
        description: impl Into<String>,
        risk: ToolRisk,
        input_schema_json: impl Into<String>,
    ) -> Self {
        Self::new(
            name,
            namespace,
            description,
            risk,
            ToolSource::BuiltIn,
            ToolExposure::Auto,
            input_schema_json,
        )
    }

    pub fn validate_input_schema(&self) -> Result<(), String> {
        let value: serde_json::Value = serde_json::from_str(&self.input_schema_json)
            .map_err(|error| format!("invalid JSON schema for {}: {error}", self.name))?;
        if value.get("type").and_then(serde_json::Value::as_str) != Some("object") {
            return Err(format!(
                "tool {} input schema must describe an object",
                self.name
            ));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_input_schema()?;
        if self
            .postcondition_verifiers
            .contains(&PostconditionVerifierKind::WorkspaceExactReadbackV1)
            && (!matches!(self.risk, ToolRisk::ReadOnly)
                || !matches!(self.effect_semantics, ToolEffectSemantics::ReadOnly))
        {
            return Err(format!(
                "tool {} may verify exact readback only with read-only risk and effect semantics",
                self.name
            ));
        }
        if self
            .postcondition_verifiers
            .contains(&PostconditionVerifierKind::WorkspaceQualityCheckV1)
            && !matches!(self.risk, ToolRisk::ExecutesProcess)
        {
            return Err(format!(
                "tool {} may verify quality checks only when it executes a process",
                self.name
            ));
        }
        if matches!(
            self.execution_concurrency,
            ToolExecutionConcurrency::IndependentRead
        ) && (!matches!(self.risk, ToolRisk::ReadOnly)
            || !matches!(self.effect_semantics, ToolEffectSemantics::ReadOnly))
        {
            return Err(format!(
                "tool {} may use independent-read concurrency only with read-only risk and effect semantics",
                self.name
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    pub id: ToolCallId,
    pub task_id: TaskId,
    pub tool_name: String,
    pub input_json: String,
    pub proposed_by_model: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcomeStatus {
    Succeeded,
    Failed,
    Cancelled,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolContent {
    Text(String),
    Image { mime_type: String, data: String },
    Resource { uri: String, text: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolArtifact {
    pub path: String,
    pub mime_type: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolObservationV2 {
    pub schema: String,
    pub tool_name: String,
    pub summary: String,
    pub evidence: String,
    pub evidence_complete: bool,
    pub facts: Metadata,
    pub next_action: Option<String>,
}

impl ToolObservationV2 {
    pub fn new(
        tool_name: impl Into<String>,
        summary: impl Into<String>,
        evidence: impl Into<String>,
        evidence_complete: bool,
        facts: Metadata,
    ) -> Self {
        Self {
            schema: TOOL_OBSERVATION_V2_SCHEMA.to_string(),
            tool_name: tool_name.into(),
            summary: summary.into(),
            evidence: evidence.into(),
            evidence_complete,
            facts,
            next_action: None,
        }
    }

    pub fn with_next_action(mut self, next_action: impl Into<String>) -> Self {
        self.next_action = Some(next_action.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub invocation_id: ToolCallId,
    pub status: ToolOutcomeStatus,
    pub output: String,
    pub content: Vec<ToolContent>,
    pub structured_output_json: Option<String>,
    pub artifacts: Vec<ToolArtifact>,
    pub failure: Option<ToolFailure>,
    pub model_observation: Option<ToolObservationV2>,
    pub metadata: Metadata,
}

impl ToolResult {
    pub fn text(
        invocation_id: ToolCallId,
        status: ToolOutcomeStatus,
        output: impl Into<String>,
        metadata: Metadata,
    ) -> Self {
        let output = output.into();
        let failure = if matches!(status, ToolOutcomeStatus::Failed) {
            Some(ToolFailure {
                code: "tool_execution_failed".to_string(),
                message: output.clone(),
                retryable: false,
            })
        } else {
            None
        };
        Self {
            invocation_id,
            status,
            content: vec![ToolContent::Text(output.clone())],
            output,
            structured_output_json: None,
            artifacts: Vec::new(),
            failure,
            model_observation: None,
            metadata,
        }
    }

    pub fn failed(invocation_id: ToolCallId, error: impl Into<String>) -> Self {
        Self::text(
            invocation_id,
            ToolOutcomeStatus::Failed,
            error,
            Metadata::new(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRole {
    Planner,
    Executor,
    Reviewer,
    Summarizer,
    Embedder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProfile {
    pub provider: String,
    pub model: String,
    pub role: ModelRole,
    pub context_window: Option<u64>,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub metadata: Metadata,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(risk: ToolRisk) -> ToolSpec {
        ToolSpec::builtin(
            "test.tool",
            "test",
            "test tool",
            risk,
            r#"{"type":"object","properties":{}}"#,
        )
    }

    #[test]
    fn tool_specs_default_to_serial_execution() {
        assert_eq!(
            spec(ToolRisk::ReadOnly).execution_concurrency,
            ToolExecutionConcurrency::Serialized
        );
    }

    #[test]
    fn independent_read_concurrency_requires_read_only_risk_and_effect() {
        assert!(spec(ToolRisk::ReadOnly)
            .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
            .validate()
            .is_ok());

        let write_error = spec(ToolRisk::WritesWorkspace)
            .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
            .validate()
            .expect_err("write tools must remain serialized");
        assert!(write_error.contains("read-only risk and effect semantics"));

        let effect_error = spec(ToolRisk::ReadOnly)
            .with_effect_semantics(ToolEffectSemantics::Idempotent)
            .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
            .validate()
            .expect_err("non-read-only effects must remain serialized");
        assert!(effect_error.contains("read-only risk and effect semantics"));
    }

    #[test]
    fn postcondition_verifier_kinds_are_closed_and_labeled() {
        assert_eq!(
            PostconditionVerifierKind::WorkspaceExactReadbackV1.label(),
            "workspace_exact_readback_v1"
        );
        assert_eq!(
            PostconditionVerifierKind::WorkspaceQualityCheckV1.label(),
            "workspace_quality_check_v1"
        );
        assert!(spec(ToolRisk::ReadOnly).postcondition_verifiers.is_empty());
    }

    #[test]
    fn postcondition_verifier_capabilities_must_match_tool_risk() {
        assert!(spec(ToolRisk::ReadOnly)
            .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceExactReadbackV1)
            .validate()
            .is_ok());
        assert!(spec(ToolRisk::ExecutesProcess)
            .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceQualityCheckV1)
            .validate()
            .is_ok());

        let readback_error = spec(ToolRisk::WritesWorkspace)
            .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceExactReadbackV1)
            .validate()
            .expect_err("write tools cannot claim exact readback evidence");
        assert!(readback_error.contains("exact readback"));

        let quality_error = spec(ToolRisk::ReadOnly)
            .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceQualityCheckV1)
            .validate()
            .expect_err("read-only tools cannot claim process quality evidence");
        assert!(quality_error.contains("quality checks"));
    }
}
