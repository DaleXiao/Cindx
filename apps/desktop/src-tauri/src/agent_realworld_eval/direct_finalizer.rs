use crate::agent_finalizer_runtime::{
    prepare_finalizer_turn, resolve_finalizer_response, FinalizerResolution,
};
use agent_core::{Message, MessageRole, Metadata, ModelRole};
use agent_runtime::{
    ensure_terminal_commit_instruction, AgentFailure, AgentLoopState, AgentTaskStateSnapshot,
    ContextGovernorReport, PreparedAgentTurn,
};
use model_provider::{ModelCallMode, ModelRequest, ModelResponse};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) const DIRECT_FINALIZER_MATCHED_PAIR_SCHEMA: &str =
    "cindx.agent-eval.direct-finalizer-matched-pair.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DirectFinalizerEvalConfig {
    pub provider_id: String,
    pub model_id: String,
    pub system_prompt: Option<String>,
    pub runtime_context: Option<String>,
    pub context_window_tokens: u64,
    pub max_output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DirectFinalizerArmReceipt {
    pub arm: &'static str,
    pub pre_treatment_sha256: String,
    pub task_contract_sha256: String,
    /// Canonical evaluator envelope, not a provider-specific wire-body digest.
    pub canonical_request_sha256: String,
    pub directive_sha256: Option<String>,
}

pub(super) struct PreparedDirectFinalizerArm {
    runtime: AgentLoopState,
    prepared: PreparedAgentTurn,
    steer_epoch: u64,
    pub receipt: DirectFinalizerArmReceipt,
}

impl PreparedDirectFinalizerArm {
    pub fn request(&self) -> &ModelRequest {
        &self.prepared.request
    }

    fn context(&self) -> &ContextGovernorReport {
        &self.prepared.context
    }

    pub fn resolve(mut self, response: ModelResponse) -> Result<FinalizerResolution, AgentFailure> {
        resolve_finalizer_response(
            &mut self.runtime,
            response,
            None,
            self.steer_epoch,
            &self.prepared.visible_contract_evidence_sequences,
        )
    }
}

pub(super) struct PreparedDirectFinalizerPair {
    pub parent: PreparedDirectFinalizerArm,
    pub challenger: PreparedDirectFinalizerArm,
}

pub(super) fn prepare_direct_finalizer_pair(
    mut canonical_runtime: AgentLoopState,
    config: DirectFinalizerEvalConfig,
    parent_directive: Option<&str>,
    challenger_directive: Option<&str>,
) -> Result<PreparedDirectFinalizerPair, String> {
    validate_config(&config)?;
    ensure_terminal_commit_instruction(&mut canonical_runtime);
    let snapshot = AgentTaskStateSnapshot::capture(&canonical_runtime);
    let task_state_json = snapshot.to_json().map_err(|error| error.to_string())?;
    let pre_treatment_sha256 = pre_treatment_sha256(&canonical_runtime, &task_state_json, &config)?;
    let task_contract_sha256 = sha256_json(&snapshot.task_contract)?;
    let steer_epoch = canonical_runtime.prepared_task_state().steer_epoch();

    let parent = prepare_arm(
        "parent",
        canonical_runtime.clone(),
        &config,
        parent_directive,
        steer_epoch,
        &pre_treatment_sha256,
        &task_contract_sha256,
    )?;
    let challenger = prepare_arm(
        "challenger",
        canonical_runtime,
        &config,
        challenger_directive,
        steer_epoch,
        &pre_treatment_sha256,
        &task_contract_sha256,
    )?;
    validate_matched_pair(&parent, &challenger)?;

    Ok(PreparedDirectFinalizerPair { parent, challenger })
}

fn validate_config(config: &DirectFinalizerEvalConfig) -> Result<(), String> {
    if config.provider_id.trim().is_empty() || config.model_id.trim().is_empty() {
        return Err("direct-finalizer evaluation requires a fixed provider and model".to_string());
    }
    if config.context_window_tokens == 0 || config.max_output_tokens == 0 {
        return Err("direct-finalizer evaluation requires non-zero token budgets".to_string());
    }
    Ok(())
}

fn prepare_arm(
    arm: &'static str,
    mut runtime: AgentLoopState,
    config: &DirectFinalizerEvalConfig,
    directive: Option<&str>,
    steer_epoch: u64,
    pre_treatment_sha256: &str,
    expected_task_contract_sha256: &str,
) -> Result<PreparedDirectFinalizerArm, String> {
    let mut prepared = prepare_finalizer_turn(
        &mut runtime,
        config.system_prompt.as_deref(),
        directive,
        config.runtime_context.as_deref(),
        config.context_window_tokens,
        config.max_output_tokens,
    )
    .map_err(|error| format!("failed to prepare {arm} direct finalizer: {error}"))?;
    prepared.request.metadata.insert(
        "max_output_tokens".to_string(),
        config.max_output_tokens.to_string(),
    );
    if prepared.request.role != ModelRole::Summarizer || !prepared.request.tools.is_empty() {
        return Err(format!(
            "{arm} direct-finalizer request escaped the tool-free summarizer contract"
        ));
    }
    let observed_contract_sha256 = sha256_json(&runtime.task_contract)?;
    if observed_contract_sha256 != expected_task_contract_sha256 {
        return Err(format!(
            "{arm} direct-finalizer preparation mutated the task contract"
        ));
    }
    let canonical_request_sha256 = canonical_request_sha256(config, &prepared.request)?;
    let directive_sha256 = directive
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| sha256_bytes(value.as_bytes()));

    Ok(PreparedDirectFinalizerArm {
        runtime,
        prepared,
        steer_epoch,
        receipt: DirectFinalizerArmReceipt {
            arm,
            pre_treatment_sha256: pre_treatment_sha256.to_string(),
            task_contract_sha256: observed_contract_sha256,
            canonical_request_sha256,
            directive_sha256,
        },
    })
}

fn validate_matched_pair(
    parent: &PreparedDirectFinalizerArm,
    challenger: &PreparedDirectFinalizerArm,
) -> Result<(), String> {
    if parent.receipt.pre_treatment_sha256 != challenger.receipt.pre_treatment_sha256
        || parent.receipt.task_contract_sha256 != challenger.receipt.task_contract_sha256
        || parent.steer_epoch != challenger.steer_epoch
    {
        return Err("direct-finalizer arms do not share one canonical pre-treatment state".into());
    }
    if parent.receipt.canonical_request_sha256 == challenger.receipt.canonical_request_sha256 {
        return Err("direct-finalizer treatment produced no request-level effect".into());
    }
    validate_request_difference(parent.request(), challenger.request())?;
    validate_context_match(parent.context(), challenger.context())
}

fn validate_request_difference(
    parent: &ModelRequest,
    challenger: &ModelRequest,
) -> Result<(), String> {
    if parent.role != challenger.role
        || parent.mode != challenger.mode
        || !parent.tools.is_empty()
        || !challenger.tools.is_empty()
        || stable_metadata(&parent.metadata) != stable_metadata(&challenger.metadata)
        || parent.messages.len() != challenger.messages.len()
    {
        return Err("direct-finalizer treatment changed the request contract".into());
    }

    let mut system_prompt_differences = 0usize;
    for (parent_message, challenger_message) in parent.messages.iter().zip(&challenger.messages) {
        if parent_message.role != challenger_message.role
            || parent_message.metadata != challenger_message.metadata
        {
            return Err("direct-finalizer treatment changed projected message identity".into());
        }
        if parent_message.content != challenger_message.content {
            if parent_message.role != MessageRole::System {
                return Err("direct-finalizer treatment changed non-system context".into());
            }
            system_prompt_differences += 1;
        }
    }
    if system_prompt_differences != 1 {
        return Err("direct-finalizer treatment must change exactly one system prompt".into());
    }
    Ok(())
}

fn validate_context_match(
    parent: &ContextGovernorReport,
    challenger: &ContextGovernorReport,
) -> Result<(), String> {
    let matched = parent.applied == challenger.applied
        && parent.repair_attempted == challenger.repair_attempted
        && parent.repair_succeeded == challenger.repair_succeeded
        && parent.context_window_tokens == challenger.context_window_tokens
        && parent.input_budget_tokens == challenger.input_budget_tokens
        && parent.original_messages == challenger.original_messages
        && parent.projected_messages == challenger.projected_messages
        && parent.omitted_messages == challenger.omitted_messages
        && parent.truncated_messages == challenger.truncated_messages
        && parent.hard_limit_satisfied == challenger.hard_limit_satisfied
        && parent.selected_context_sources == challenger.selected_context_sources
        && parent.omitted_context_sources == challenger.omitted_context_sources
        && parent.current_request_preserved == challenger.current_request_preserved
        && parent.protected_sources_satisfied == challenger.protected_sources_satisfied
        && parent.tool_round_integrity_satisfied == challenger.tool_round_integrity_satisfied;
    matched
        .then_some(())
        .ok_or_else(|| "direct-finalizer treatment changed context selection or invariants".into())
}

fn stable_metadata(metadata: &Metadata) -> BTreeMap<&str, &str> {
    metadata
        .iter()
        .filter(|(key, _)| !key.starts_with("context_"))
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreTreatmentDigest<'a> {
    schema: &'static str,
    task_state_json: &'a str,
    messages: Vec<CanonicalMessage<'a>>,
    provider_id: &'a str,
    model_id: &'a str,
    system_prompt: &'a Option<String>,
    runtime_context: &'a Option<String>,
    context_window_tokens: u64,
    max_output_tokens: u64,
}

fn pre_treatment_sha256(
    runtime: &AgentLoopState,
    task_state_json: &str,
    config: &DirectFinalizerEvalConfig,
) -> Result<String, String> {
    sha256_json(&PreTreatmentDigest {
        schema: DIRECT_FINALIZER_MATCHED_PAIR_SCHEMA,
        task_state_json,
        messages: runtime
            .messages
            .iter()
            .map(CanonicalMessage::from)
            .collect(),
        provider_id: &config.provider_id,
        model_id: &config.model_id,
        system_prompt: &config.system_prompt,
        runtime_context: &config.runtime_context,
        context_window_tokens: config.context_window_tokens,
        max_output_tokens: config.max_output_tokens,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestPayloadDigest<'a> {
    schema: &'static str,
    provider_id: &'a str,
    model_id: &'a str,
    role: &'static str,
    mode: &'static str,
    messages: Vec<CanonicalMessage<'a>>,
    tools: Vec<&'a str>,
    metadata: &'a Metadata,
}

fn canonical_request_sha256(
    config: &DirectFinalizerEvalConfig,
    request: &ModelRequest,
) -> Result<String, String> {
    sha256_json(&RequestPayloadDigest {
        schema: "cindx.agent-eval.direct-finalizer-request-payload.v1",
        provider_id: &config.provider_id,
        model_id: &config.model_id,
        role: model_role(request.role.clone()),
        mode: model_call_mode(&request.mode),
        messages: request
            .messages
            .iter()
            .map(CanonicalMessage::from)
            .collect(),
        tools: request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect(),
        metadata: &request.metadata,
    })
}

#[derive(Serialize)]
struct CanonicalMessage<'a> {
    role: &'static str,
    content: &'a str,
    metadata: &'a Metadata,
}

impl<'a> From<&'a Message> for CanonicalMessage<'a> {
    fn from(message: &'a Message) -> Self {
        Self {
            role: message_role(&message.role),
            content: &message.content,
            metadata: &message.metadata,
        }
    }
}

fn message_role(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::Reviewer => "reviewer",
    }
}

fn model_role(role: ModelRole) -> &'static str {
    match role {
        ModelRole::Planner => "planner",
        ModelRole::Executor => "executor",
        ModelRole::Reviewer => "reviewer",
        ModelRole::Summarizer => "summarizer",
        ModelRole::Embedder => "embedder",
    }
}

fn model_call_mode(mode: &ModelCallMode) -> &'static str {
    match mode {
        ModelCallMode::NonStreaming => "non_streaming",
        ModelCallMode::Streaming => "streaming",
    }
}

fn sha256_json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| sha256_bytes(&bytes))
        .map_err(|error| format!("failed to encode direct-finalizer digest: {error}"))
}

fn sha256_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
