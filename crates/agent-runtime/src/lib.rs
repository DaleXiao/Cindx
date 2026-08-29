use agent_core::{
    tool_function_name, Message, MessageRole, Metadata, ModelCallMode, ModelRequest, ModelResponse,
    ModelResponseDisposition, ModelRole, TaskId, ToolCallId, ToolInvocation, ToolOutcomeStatus,
    ToolPostconditionEvidence, ToolResult, ToolRisk, ToolSpec, TOOL_OBSERVATION_V2_SCHEMA,
};
use std::collections::{BTreeMap, BTreeSet};
pub use system_prompt::{
    agent_system_prompt_with_context, agent_system_prompt_with_override,
    compose_agent_system_prompt, compose_base_agent_system_prompt,
};

const DSML_TOOL_CALLS_OPEN: &str = "<｜DSML｜tool_calls>";
const DSML_TOOL_CALLS_CLOSE: &str = "</｜DSML｜tool_calls>";

mod adaptive_loop;
mod anytime_parallel;
mod completion_intent;
mod context_compiler;
mod context_engine;
mod context_governor;
mod context_projection;
mod context_token_ledger;
mod control;
mod control_steer;
mod delivery_verification;
mod effect_instruction_segments;
mod evidence_target;
mod execution;
mod failure;
mod grounded_context;
mod grounding_policy;
#[cfg(test)]
mod grounding_policy_tests;
mod grounding_tools;
mod kernel;
mod loop_observers;
mod model_transport;
mod parallel;
mod prepared_task_state;
mod repetition_advisory;
mod resource_ledger;
mod result_frontier;
mod run_budget;
mod run_context;
mod state_transaction;
mod subagent;
mod system_prompt;
mod task_contract;
mod task_state;
mod task_state_lineage;
mod task_state_wire;
mod token_counter;
mod tool_runtime;
mod turn_budget;
mod worker_policy;
mod worker_runtime;
mod worker_tools;

pub use adaptive_loop::{AdaptiveLoopCursor, AdaptiveLoopDisposition};
pub use anytime_parallel::{AnytimeQuorumExecution, AnytimeQuorumPolicy};
pub use completion_intent::{
    prompt_completion_intent, prompt_evidence_target_anchors, prompt_replaces_prior_objective,
    PromptCompletionIntent, PromptEffectAuthority, PromptToolRequirement,
};
pub use context_compiler::{
    ContextCompilerCounts, ContextCompilerHardInvariants, ContextCompilerOperationCounts,
    ContextCompilerReceipt, CONTEXT_COMPILER_POLICY, CONTEXT_COMPILER_RECEIPT_SCHEMA,
    MAX_CONTEXT_COMPILER_RECEIPT_BYTES,
};
pub use context_engine::{
    compaction_summary_instruction, context_prompt_reserve, estimate_context_tokens,
    estimate_message_tokens, estimate_request_tokens, estimate_text_tokens,
    extractive_rolling_summary, is_balanced_cut, is_user_turn_start, nearest_balanced_cut,
    serialize_transcript_for_compaction, ContextCompactionPlan, ContextCompactionPolicy,
    ContextEngine, ContextSourceKind, CONTEXT_SOURCE_SCHEMA,
};
pub use context_governor::{
    bounded_max_output_tokens, ContextBudgetAllocation, ContextGovernorReport,
    ContextInvariantViolation,
};
pub use control::{
    AgentRunControl, RunContinuationDirective, RunControlSnapshot, RunEpochLease,
    RunEpochLeaseOutcome, RunExecutionStepCommit, RunPreparationCheckpoint, RunPreparationCommit,
    RunProgressSnapshot, RunStageUsageSnapshot, RunStartCheckpoint, RunSteer, RunSteerBatchCommit,
    RunSteerRequestCommit, RunStopReason, RunTelemetryCountersSnapshot, RunTerminalCommit,
    RunToolCallBatchStart, RunToolCallStart,
};
pub use delivery_verification::*;
pub use evidence_target::{
    evidence_input_matches_anchors, evidence_target_anchors, evidence_target_witness,
    EvidenceTargetAnchor,
};
pub use execution::{
    run_no_tool_agent, AgentEvidenceCandidate, AgentEvidencePacket, AgentExecutionGuidance,
    NoToolAgentOutcome, NoToolAgentRequest, AGENT_EVIDENCE_PACKET_SCHEMA,
};
pub use failure::{AgentFailure, AgentFailureClass, AgentRecoveryAction};
pub use grounded_context::{
    message_contract_evidence_sequences, CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY,
};
pub use grounding_policy::{prompt_evidence_scopes, PromptEvidenceScope};
pub use grounding_tools::{
    pin_evidence_scope_tools, pin_prompt_evidence_tools, tool_matches_evidence_scope,
};
pub use kernel::{
    AgentKernel, AgentKernelInstruction, AgentKernelInstructionKind,
    AgentToolObservationTransition, AgentTurnPreparationError, GroundedCompletionDecision,
    PreparedAgentTurn,
};
pub use loop_observers::{
    clear_doom_loop_confirmation, generic_hint_message, http_hosts_for_tool_io, normalize_host,
    observation_excerpt, quarantined_web_target_host, stuck_target_blocked_observation,
    DoomLoopObserver, FinalizationObserver, Intervention, LoopObserver, LoopObservers,
    ObserverContext, ObserverEmission, ObserverRegistry, RepetitionAdvisoryObserver,
    StuckTargetObserver, DOOM_LOOP_CONFIRMATION_KIND, DOOM_LOOP_CONFIRMATION_THRESHOLD,
    FORCE_FINAL_TURN_INSTRUCTION, LOOP_OBSERVER_HINT_KIND, STUCK_TARGET_BLOCKED_CODE,
    STUCK_TARGET_QUARANTINE_THRESHOLD, STUCK_TARGET_TOOLS,
};
pub use model_transport::{
    exhausted_model_transport_error_stop_reason, model_response_checkpoint_evidence,
    model_transport_retry_delay, ModelStreamProgress,
};
pub use parallel::{
    BoundedParallelExecutor, CancellableParallelJob, InterruptibleQuorumExecution,
    InterruptibleQuorumPolicy, ParallelJob, ParallelJobCompletion, ParallelJobSupervisor,
    ParallelTaskError, QuorumExecution,
};
pub use prepared_task_state::PreparedTaskState;
pub use repetition_advisory::{
    RepetitionAdvisoryTracker, REPETITION_ADVISORY_KIND, REPETITION_ADVISORY_PREVIEW_MAX_CHARS,
    REPETITION_ADVISORY_THRESHOLDS,
};
pub use resource_ledger::{
    ModelAttemptUsage, ModelResourceUsage, ModelUsageSource, ModelUsageSourceCounts,
    PhysicalModelAttempt, RunResourceSnapshot, RunResourceUsage, MAX_PENDING_RESOURCE_ATTEMPTS,
    MAX_PERSISTED_RESOURCE_SNAPSHOT_BYTES, MAX_RESOURCE_LEDGER_MODELS,
    MAX_RESOURCE_MODEL_KEY_BYTES,
};
pub use result_frontier::{BestKnownResult, ResultQuality};
pub use run_budget::{
    RunBudget, RunStageBudget, RunStageClass, CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT,
    PHYSICAL_MODEL_ATTEMPTS_PER_LOGICAL_CALL,
};
pub use run_context::{effective_agent_objective, run_context_steer_epoch};
pub use state_transaction::AgentLoopAppendTransaction;
pub use subagent::{
    build_subagent_task_prompt, subagent_context_fork_prefix, subagent_patch_tool_allowed,
    subagent_system_prompt, subagent_tool_allowed, subagent_write_system_prompt,
    SUBAGENT_ALLOWED_TOOLS, SUBAGENT_CONTEXT_FORK_MAX_MESSAGES, SUBAGENT_MAX_STEPS,
    SUBAGENT_PATCH_TOOLS,
};
pub use task_contract::{
    AgentActionDenial, AgentActionDenialFeedback, AgentActionDenialKind, AgentActionDenialScope,
    AgentActionRecovery, AgentCognitiveFocus, AgentCognitiveState, AgentGoalDelta,
    AgentGoalDeltaKind, AgentTaskContract, ContractEvidence, ContractEvidenceKind,
    GroundedCompletionBasis, GroundedCompletionIssue, GroundedCompletionReceipt, OutcomeBlocker,
    OutcomeClaim, OutcomeClaimDecision, OutcomeClaimEvidenceStatus, OutcomeClaimKind,
    OutcomeClaimQuality, OutcomeEvidence, OutcomeFailure, OutcomeFailureClass, OutcomeLedgerPhase,
    OutcomeLedgerShadow, OutcomeObligation, OutcomeObligationKind, OutcomePostcondition,
    OutcomePostconditionKind, OutcomePostconditionStatus, OutcomeSatisfaction, OutcomeScope,
    OutcomeTerminal, OutcomeTerminalObservation, OutcomeTruncation,
    PostconditionVerificationReceipt, PromptEvidenceContext, WorkspaceVerificationPolicy,
    ACTION_DENIAL_SCHEMA, COGNITIVE_STATE_MAX_BYTES, COGNITIVE_STATE_SCHEMA, GOAL_DELTA_SCHEMA,
    GROUNDED_COMPLETION_DIGEST_METADATA_KEY, GROUNDED_COMPLETION_METADATA_KEY,
    GROUNDED_COMPLETION_SCHEMA, MAX_POSTCONDITION_VERIFICATION_RECEIPT_BYTES,
    OUTCOME_LEDGER_DIGEST_METADATA_KEY, OUTCOME_LEDGER_MAX_METADATA_BYTES,
    OUTCOME_LEDGER_METADATA_KEY, OUTCOME_LEDGER_SCHEMA,
    POSTCONDITION_VERIFICATION_DIGEST_METADATA_KEY, POSTCONDITION_VERIFICATION_METADATA_KEY,
    POSTCONDITION_VERIFICATION_SCHEMA,
};
pub use task_state::{
    AgentTaskStateError, AgentTaskStateSnapshot, AGENT_TASK_STATE_SCHEMA,
    AGENT_TASK_STATE_SCHEMA_V1, MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES,
};
pub use task_state_lineage::{AgentTaskStateLineage, AgentTranscriptFingerprintAccumulator};
pub use task_state_wire::{
    PersistedInteractionSurface, PersistedInteractionVerification, PreparedTaskStateCheckpoint,
};
pub use token_counter::{
    count_text_tokens, token_counter_kind, Cl100kTokenCounter, HeuristicTokenCounter, TokenCounter,
};
pub use tool_runtime::{
    apply_tool_spec_runtime_metadata, decode_persisted_tool_artifacts,
    decode_persisted_tool_model_observation, finalize_tool_result, postcondition_lineage_scope,
    recovery_source_scope_matches, supports_recovery_effect_replay, tool_effect_recovery_policy,
    tool_execution_scope_matches, tool_input_fingerprint, tool_invocation_context,
    tool_invocation_event_metadata, tool_risk_label, PersistedToolEffectKind,
    PersistedToolEffectWitness, ToolEffectRecoveryPolicy, EFFECT_LEDGER_SCHEMA,
    MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES, TOOL_EFFECT_SEMANTICS_METADATA_KEY,
    TOOL_EFFECT_VERIFIER_METADATA_KEY, TOOL_EFFECT_WITNESS_METADATA_KEY,
    TOOL_EFFECT_WITNESS_SCHEMA, TOOL_MODEL_OBSERVATION_METADATA_KEY, TOOL_RESULT_SCHEMA,
    TOOL_RISK_METADATA_KEY,
};
pub use turn_budget::AgentTurnBudgetExhausted;
pub use worker_policy::{WorkerTurnPhase, WorkerTurnPolicy};
pub use worker_runtime::{
    IsolatedWorkerRuntime, PreparedWorkerTurn, WorkerAdvance, WorkerFailure, WorkerToolAdmission,
    WorkerToolDenialKind,
};
pub use worker_tools::{evidence_worker_tools, substantive_evidence_worker_tools};

pub const DEFAULT_MAX_AGENT_TURNS: usize = 24;
pub const DEFAULT_COLLABORATION_WORKER_TURNS: usize = 5;
pub const MAX_COLLABORATION_WORKER_TOOL_CALLS: usize = 6;
pub const MAX_IDENTICAL_TOOL_FAILURES: usize = 2;
pub(crate) const TOOL_FAILURE_SIGNATURE_SCHEMA: &str = "cindx.tool-failure.v1";
pub const CORE_AGENT_SYSTEM_PROMPT: &str = include_str!("core_prompt.txt");
pub const DEFAULT_AGENT_SYSTEM_PROMPT: &str = CORE_AGENT_SYSTEM_PROMPT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntimeConfig {
    pub max_turns: usize,
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_AGENT_TURNS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLoopState {
    pub task_id: TaskId,
    pub user_prompt: String,
    pub messages: Vec<Message>,
    pub turn: usize,
    pub max_turns: usize,
    pub failed_tool_signatures: BTreeMap<String, usize>,
    pub consecutive_empty_responses: usize,
    pub verification_gate_requests: usize,
    pub verified_interactions: usize,
    pub interaction_verification_gate_requests: usize,
    pub task_contract: AgentTaskContract,
    prepared_task_state: PreparedTaskState,
    adaptive_loop_cursor: AdaptiveLoopCursor,
    context_token_ledger: context_token_ledger::ContextTokenLedger,
    /// Consecutive identical tool-call streak used for the advisory repetition
    /// reminder. Runtime-only; restored runs start with a fresh streak.
    pub repetition_advisory: RepetitionAdvisoryTracker,
    /// Advisory loop observers and their runtime-only intervention effects
    /// (host quarantine, forced-final-turn flag, pending doom-loop
    /// confirmation). Runtime-only; restored runs start with a fresh host.
    pub loop_observers: LoopObservers,
    pub generation_temperature: Option<String>,
    /// The run's reasoning level (fast/default/high/xhigh); maps to the
    /// provider's thinking/reasoning effort parameter.
    pub reasoning_effort: Option<String>,
}

impl AgentLoopState {
    pub fn replace_prepared_task_state(&mut self, prepared_task_state: PreparedTaskState) {
        if prepared_task_state.steer_epoch() != self.prepared_task_state.steer_epoch()
            || prepared_task_state.contract_epoch() != self.prepared_task_state.contract_epoch()
            || prepared_task_state.objective_fingerprint()
                != self.prepared_task_state.objective_fingerprint()
        {
            self.adaptive_loop_cursor
                .reset_for_steer(prepared_task_state.steer_epoch());
        }
        self.prepared_task_state = prepared_task_state;
    }

    pub fn prepared_task_state(&self) -> &PreparedTaskState {
        &self.prepared_task_state
    }

    pub fn advance_prepared_task_control_epoch(&mut self, steer_epoch: u64) {
        let prior_epoch = self.prepared_task_state.steer_epoch();
        self.prepared_task_state.advance_control_epoch(steer_epoch);
        if self.prepared_task_state.steer_epoch() != prior_epoch {
            self.adaptive_loop_cursor
                .reset_for_steer(self.prepared_task_state.steer_epoch());
        }
    }

    pub fn successful_mutations(&self) -> usize {
        self.task_contract.successful_mutations()
    }

    pub fn verified_after_last_mutation(&self) -> bool {
        self.task_contract.latest_mutation_verified()
    }

    pub fn pending_interaction_verifications(&self) -> &BTreeMap<InteractionSurface, String> {
        self.task_contract.pending_interactions()
    }

    /// Invalidates cached token estimates after an in-place mutation to an
    /// estimator-relevant message field (`content`, `raw_tool_calls_json`, or
    /// `image_paths`). Structural replacements are detected automatically.
    pub fn invalidate_context_token_estimates_from(&mut self, first_changed_message: usize) {
        self.context_token_ledger
            .invalidate_from(first_changed_message);
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum InteractionSurface {
    Browser,
    Computer,
}

impl InteractionSurface {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Computer => "computer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentToolRequest {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAdvance {
    Completed { answer: String },
    ToolCalls { calls: Vec<AgentToolRequest> },
    TurnBudgetExhausted(AgentTurnBudgetExhausted),
    Retry { instruction: String },
    Failed { failure: AgentFailure },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkTag {
    Open,
    Close,
}

fn think_tag_at(bytes: &[u8], index: usize) -> Option<(ThinkTag, usize)> {
    if bytes.get(index) != Some(&b'<') {
        return None;
    }

    let mut cursor = index + 1;
    let tag = if bytes.get(cursor) == Some(&b'/') {
        cursor += 1;
        ThinkTag::Close
    } else {
        ThinkTag::Open
    };
    let name_end = cursor.checked_add(5)?;
    if name_end > bytes.len() || !bytes[cursor..name_end].eq_ignore_ascii_case(b"think") {
        return None;
    }
    cursor = name_end;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    (bytes.get(cursor) == Some(&b'>')).then_some((tag, cursor + 1 - index))
}

fn delimiter_run_length(bytes: &[u8], index: usize, delimiter: u8) -> usize {
    bytes[index..]
        .iter()
        .take_while(|byte| **byte == delimiter)
        .count()
}

fn code_span_end(bytes: &[u8], start: usize, delimiter: u8, run_length: usize) -> Option<usize> {
    let mut cursor = start + run_length;
    while cursor < bytes.len() {
        if bytes[cursor] != delimiter {
            cursor += 1;
            continue;
        }
        let closing_length = delimiter_run_length(bytes, cursor, delimiter);
        let closes_span = if run_length >= 3 {
            closing_length >= run_length
        } else {
            closing_length == run_length
        };
        if closes_span {
            return Some(cursor + closing_length);
        }
        cursor += closing_length;
    }
    None
}

/// Removes provider reasoning control markup without exposing or retaining hidden reasoning.
/// Markdown inline code and fenced code blocks are preserved verbatim.
pub fn sanitize_assistant_content(content: &str) -> String {
    let bytes = content.as_bytes();
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;
    let mut inside_think = false;

    while cursor < bytes.len() {
        if let Some((tag, length)) = think_tag_at(bytes, cursor) {
            match tag {
                ThinkTag::Open => inside_think = true,
                ThinkTag::Close => inside_think = false,
            }
            cursor += length;
            continue;
        }

        if inside_think {
            cursor += 1;
            continue;
        }

        let delimiter = bytes[cursor];
        if delimiter == b'`' || delimiter == b'~' {
            let run_length = delimiter_run_length(bytes, cursor, delimiter);
            if delimiter == b'`' || run_length >= 3 {
                if let Some(end) = code_span_end(bytes, cursor, delimiter, run_length) {
                    output.push_str(&content[cursor..end]);
                    cursor = end;
                    continue;
                }
            }
        }

        if content[cursor..].starts_with(DSML_TOOL_CALLS_OPEN) {
            let body_start = cursor + DSML_TOOL_CALLS_OPEN.len();
            cursor = content[body_start..]
                .find(DSML_TOOL_CALLS_CLOSE)
                .map(|offset| body_start + offset + DSML_TOOL_CALLS_CLOSE.len())
                .unwrap_or(content.len());
            continue;
        }

        let character = content[cursor..]
            .chars()
            .next()
            .expect("cursor stays on a character boundary");
        output.push(character);
        cursor += character.len_utf8();
    }

    output.trim().to_string()
}

fn initial_user_message(content: String) -> Message {
    Message {
        role: MessageRole::User,
        content,
        metadata: Metadata::new(),
    }
}

pub fn start_agent_loop(
    task_id: TaskId,
    user_prompt: impl Into<String>,
    config: AgentRuntimeConfig,
) -> AgentLoopState {
    let user_prompt = user_prompt.into();
    let prepared_task_state = PreparedTaskState::initial(&user_prompt);
    AgentLoopState {
        task_id,
        messages: vec![initial_user_message(user_prompt.clone())],
        user_prompt,
        turn: 0,
        max_turns: config.max_turns.max(1),
        failed_tool_signatures: BTreeMap::new(),
        consecutive_empty_responses: 0,
        verification_gate_requests: 0,
        verified_interactions: 0,
        interaction_verification_gate_requests: 0,
        task_contract: AgentTaskContract::default(),
        prepared_task_state,
        adaptive_loop_cursor: AdaptiveLoopCursor::default(),
        context_token_ledger: Default::default(),
        repetition_advisory: RepetitionAdvisoryTracker::default(),
        loop_observers: LoopObservers::default(),
        generation_temperature: None,
        reasoning_effort: None,
    }
}

pub fn start_agent_loop_with_history(
    task_id: TaskId,
    user_prompt: impl Into<String>,
    mut history: Vec<Message>,
    config: AgentRuntimeConfig,
) -> AgentLoopState {
    let user_prompt = user_prompt.into();
    let prepared_task_state = PreparedTaskState::initial(&user_prompt);
    history.push(initial_user_message(user_prompt.clone()));
    AgentLoopState {
        task_id,
        user_prompt,
        messages: history,
        turn: 0,
        max_turns: config.max_turns.max(1),
        failed_tool_signatures: BTreeMap::new(),
        consecutive_empty_responses: 0,
        verification_gate_requests: 0,
        verified_interactions: 0,
        interaction_verification_gate_requests: 0,
        task_contract: AgentTaskContract::default(),
        prepared_task_state,
        adaptive_loop_cursor: AdaptiveLoopCursor::default(),
        context_token_ledger: Default::default(),
        repetition_advisory: RepetitionAdvisoryTracker::default(),
        loop_observers: LoopObservers::default(),
        generation_temperature: None,
        reasoning_effort: None,
    }
}

pub fn resume_agent_loop(
    task_id: TaskId,
    user_prompt: impl Into<String>,
    observations: &[String],
    config: AgentRuntimeConfig,
) -> AgentLoopState {
    let mut state = start_agent_loop(task_id, user_prompt, config);
    for observation in observations {
        append_observation(&mut state, observation);
    }
    state
}

pub fn resume_agent_loop_from_messages(
    task_id: TaskId,
    user_prompt: impl Into<String>,
    messages: Vec<Message>,
    config: AgentRuntimeConfig,
) -> AgentLoopState {
    let user_prompt = user_prompt.into();
    let prepared_task_state = PreparedTaskState::initial(&user_prompt);
    let turn = messages
        .iter()
        .filter(|message| matches!(message.role, MessageRole::Assistant))
        .count();
    let mut state = AgentLoopState {
        task_id,
        user_prompt,
        messages,
        turn,
        max_turns: config.max_turns.max(1),
        failed_tool_signatures: BTreeMap::new(),
        consecutive_empty_responses: 0,
        verification_gate_requests: 0,
        verified_interactions: 0,
        interaction_verification_gate_requests: 0,
        task_contract: AgentTaskContract::default(),
        prepared_task_state,
        adaptive_loop_cursor: AdaptiveLoopCursor::default(),
        context_token_ledger: Default::default(),
        repetition_advisory: RepetitionAdvisoryTracker::default(),
        loop_observers: LoopObservers::default(),
        generation_temperature: None,
        reasoning_effort: None,
    };
    rebuild_interaction_verification_state(&mut state);
    state
}

pub fn model_request_for_turn(state: &AgentLoopState, tools: &[ToolSpec]) -> ModelRequest {
    model_request_for_turn_with_system_prompt(state, tools, None)
}

pub fn model_request_for_turn_with_system_prompt(
    state: &AgentLoopState,
    tools: &[ToolSpec],
    user_instructions: Option<&str>,
) -> ModelRequest {
    model_request_for_turn_with_context(state, tools, user_instructions, None)
}

pub fn model_request_for_turn_with_context(
    state: &AgentLoopState,
    tools: &[ToolSpec],
    user_instructions: Option<&str>,
    runtime_context: Option<&str>,
) -> ModelRequest {
    let mut messages = vec![Message {
        role: MessageRole::System,
        content: agent_system_prompt_with_context(tools, user_instructions, runtime_context),
        metadata: Metadata::new(),
    }];
    messages.extend(state.messages.clone());

    ModelRequest {
        role: ModelRole::Executor,
        messages,
        tools: tools.to_vec(),
        mode: ModelCallMode::NonStreaming,
        metadata: [
            ("agent_task_id".to_string(), state.task_id.0.clone()),
            ("agent_turn".to_string(), state.turn.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

pub fn model_request_for_turn_with_context_budget(
    state: &mut AgentLoopState,
    tools: &[ToolSpec],
    user_instructions: Option<&str>,
    runtime_context: Option<&str>,
    context_window_tokens: u64,
    max_output_tokens: u64,
) -> (ModelRequest, ContextGovernorReport) {
    model_request_for_turn_with_context_budget_and_overlays(
        state,
        tools,
        user_instructions,
        runtime_context,
        &[],
        context_window_tokens,
        max_output_tokens,
    )
}

pub fn model_request_for_turn_with_context_budget_and_overlays(
    state: &mut AgentLoopState,
    tools: &[ToolSpec],
    user_instructions: Option<&str>,
    runtime_context: Option<&str>,
    context_overlays: &[Message],
    context_window_tokens: u64,
    max_output_tokens: u64,
) -> (ModelRequest, ContextGovernorReport) {
    let system_prompt = agent_system_prompt_with_context(tools, user_instructions, runtime_context);
    state.context_token_ledger.synchronize(&state.messages);
    let effective_objective = state.prepared_task_state.effective_objective();
    let objective_fingerprint = state.prepared_task_state.objective_fingerprint();
    let (messages, report) =
        context_governor::govern_model_messages_with_overlays_and_estimates_for_objective(
            &state.messages,
            state.context_token_ledger.tokens(),
            system_prompt,
            context_overlays,
            tools,
            context_window_tokens,
            max_output_tokens,
            effective_objective,
            objective_fingerprint,
        );
    let mut metadata = [
        ("agent_task_id".to_string(), state.task_id.0.clone()),
        ("agent_turn".to_string(), state.turn.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(temperature) = state.generation_temperature.clone() {
        metadata.insert(
            agent_core::GENERATION_TEMPERATURE_KEY.to_string(),
            temperature,
        );
    }
    if let Some(reasoning_effort) = state.reasoning_effort.clone() {
        metadata.insert(
            agent_core::REASONING_EFFORT_KEY.to_string(),
            reasoning_effort,
        );
    }
    report.insert_metadata(&mut metadata);

    (
        ModelRequest {
            role: ModelRole::Executor,
            messages,
            tools: tools.to_vec(),
            mode: ModelCallMode::NonStreaming,
            metadata,
        },
        report,
    )
}

pub fn advance_with_model_response(
    state: &mut AgentLoopState,
    response: ModelResponse,
    tools: &[ToolSpec],
) -> AgentAdvance {
    let assessment = response.assessment();
    let truncated_batch = response.truncated_tool_call_batch();
    let content = sanitize_assistant_content(&response.message.content);
    let tool_call_count = response.tool_calls.len();
    turn_budget::record_model_response(state);

    if !content.is_empty() || tool_call_count > 0 {
        let mut metadata = response.message.metadata.clone();
        for (key, value) in response.metadata.clone() {
            metadata.entry(key).or_insert(value);
        }
        metadata.insert("tool_call_count".to_string(), tool_call_count.to_string());
        metadata.insert(
            "model_response_disposition".to_string(),
            format!("{:?}", assessment.disposition).to_ascii_lowercase(),
        );
        if truncated_batch
            || matches!(
                assessment.disposition,
                ModelResponseDisposition::IncompleteOutput | ModelResponseDisposition::Filtered
            )
        {
            metadata.insert("internal".to_string(), "true".to_string());
        }
        // A truncated batch is never executed, so its call ids must not enter
        // the transcript (that would leave dangling tool calls with no results).
        if !truncated_batch {
            if let Some(raw_tool_calls_json) = response.raw_tool_calls_json.clone() {
                metadata.insert("raw_tool_calls_json".to_string(), raw_tool_calls_json);
            }
            if tool_call_count > 0 {
                metadata.insert(
                    "tool_call_ids".to_string(),
                    response
                        .tool_calls
                        .iter()
                        .map(|call| call.id.clone())
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
        }
        state.messages.push(Message {
            role: MessageRole::Assistant,
            content: content.clone(),
            metadata,
        });
    }

    let retry_instruction = if truncated_batch {
        Some(
            "The previous tool-call batch was cut off by the output limit and was NOT executed, because truncated arguments are unsafe. Re-issue the needed tool calls with complete, valid arguments (prefer fewer or smaller calls), or provide a complete final answer instead.",
        )
    } else {
        match assessment.disposition {
        ModelResponseDisposition::IncompleteOutput => Some(
            "The previous response reached its output limit before completion. Continue from the preserved partial response without repeating it. Finish the pending reasoning or make the next necessary tool call, then provide a complete answer."
        ),
        ModelResponseDisposition::Filtered => Some(
            "The previous response was blocked by the provider safety filter. Reformulate the next step in a policy-compliant way while preserving the user's legitimate goal. Do not repeat the blocked wording."
        ),
        ModelResponseDisposition::Empty => Some(
            "The previous model response was empty. Continue from the preserved task state: either make the next necessary tool call or provide a substantive final answer grounded in available evidence. Do not return an empty response."
        ),
        ModelResponseDisposition::Usable | ModelResponseDisposition::ToolCalls => None,
        }
    };
    if let Some(instruction) = retry_instruction {
        state.consecutive_empty_responses = state.consecutive_empty_responses.saturating_add(1);
        if state.turn >= state.max_turns {
            return AgentAdvance::TurnBudgetExhausted(turn_budget::turn_budget_exhaustion(
                state,
                (!content.is_empty()).then_some(content),
            ));
        }
        if state.consecutive_empty_responses <= 2 {
            return AgentAdvance::Retry {
                instruction: instruction.to_string(),
            };
        }
        return AgentAdvance::Failed {
            failure: AgentFailure::model_output(
                "repeated_unusable_model_response",
                format!(
                    "model returned three consecutive {:?} responses",
                    assessment.disposition
                )
                .to_ascii_lowercase(),
            ),
        };
    }
    state.consecutive_empty_responses = 0;

    if !response.tool_calls.is_empty() {
        if state.turn >= state.max_turns {
            return AgentAdvance::TurnBudgetExhausted(turn_budget::turn_budget_exhaustion(
                state,
                (!content.is_empty()).then_some(content),
            ));
        }
        let calls = response
            .tool_calls
            .into_iter()
            .map(|call| {
                let tool_name = original_tool_name(&call.name, tools);
                AgentToolRequest {
                    call_id: ToolCallId(call.id),
                    tool_name,
                    input: call.arguments_json,
                }
            })
            .collect::<Vec<_>>();

        if calls.is_empty() {
            return AgentAdvance::Failed {
                failure: AgentFailure::model_output(
                    "empty_tool_call_set",
                    "model returned an empty tool call set",
                ),
            };
        }

        return AgentAdvance::ToolCalls { calls };
    }

    AgentAdvance::Completed { answer: content }
}

pub fn append_internal_instruction(state: &mut AgentLoopState, kind: &str, instruction: &str) {
    state.messages.push(Message {
        role: MessageRole::System,
        content: instruction.to_string(),
        metadata: [
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), kind.to_string()),
        ]
        .into_iter()
        .collect(),
    });
}

pub fn ensure_terminal_commit_instruction(state: &mut AgentLoopState) -> bool {
    if state.messages.iter().any(|message| {
        message.metadata.get("kind").map(String::as_str) == Some("terminal_commit_policy")
    }) {
        return false;
    }
    append_internal_instruction(
        state,
        "terminal_commit_policy",
        "The run has entered its terminal reserve. Stop exploratory work. Complete or verify only an action that is already in progress and essential to the user's outcome; otherwise return the best grounded result now. Preserve every user constraint, state any unresolved limitation explicitly, and do not start a new branch of work.",
    );
    true
}

pub fn append_steering_instruction(
    state: &mut AgentLoopState,
    instruction: impl Into<String>,
    mut metadata: Metadata,
) -> usize {
    let closed_tool_calls = close_unmatched_tool_calls_for_steer(state);
    metadata.insert("steer".to_string(), "true".to_string());
    state.messages.push(Message {
        role: MessageRole::User,
        content: instruction.into(),
        metadata,
    });
    state.consecutive_empty_responses = 0;
    closed_tool_calls
}

fn close_unmatched_tool_calls_for_steer(state: &mut AgentLoopState) -> usize {
    let Some((assistant_index, assistant)) = state
        .messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| matches!(message.role, MessageRole::Assistant | MessageRole::User))
    else {
        return 0;
    };
    if !matches!(assistant.role, MessageRole::Assistant) {
        return 0;
    }
    let call_ids = assistant_tool_call_ids(assistant);
    if call_ids.is_empty() {
        return 0;
    }
    let observed = state.messages[assistant_index + 1..]
        .iter()
        .filter(|message| matches!(message.role, MessageRole::Tool))
        .filter_map(|message| message.metadata.get("tool_call_id").cloned())
        .collect::<BTreeSet<_>>();
    let unmatched = call_ids
        .into_iter()
        .filter(|call_id| !observed.contains(call_id))
        .collect::<Vec<_>>();

    for call_id in &unmatched {
        state.messages.push(Message {
            role: MessageRole::Tool,
            content: "status=cancelled\nreason=superseded_by_user_steer\noutput=\nTool call was cancelled before execution because a newer user steering instruction superseded this tool-call round.".to_string(),
            metadata: [
                ("kind".to_string(), "tool_observation".to_string()),
                ("tool_call_id".to_string(), call_id.clone()),
                ("status".to_string(), "cancelled".to_string()),
                ("synthetic".to_string(), "true".to_string()),
                (
                    "reason".to_string(),
                    "superseded_by_user_steer".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }
    unmatched.len()
}

fn assistant_tool_call_ids(message: &Message) -> Vec<String> {
    if !matches!(message.role, MessageRole::Assistant) {
        return Vec::new();
    }
    let mut seen = BTreeSet::new();
    let metadata_ids = message
        .metadata
        .get("tool_call_ids")
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|call_id| !call_id.is_empty())
        .filter(|call_id| seen.insert((*call_id).to_string()))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !metadata_ids.is_empty() {
        return metadata_ids;
    }

    message
        .metadata
        .get("raw_tool_calls_json")
        .and_then(|raw_calls| serde_json::from_str::<serde_json::Value>(raw_calls).ok())
        .and_then(|value| value.as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|call| {
            call.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|call_id| !call_id.is_empty())
                .map(str::to_string)
        })
        .filter(|call_id| seen.insert(call_id.clone()))
        .collect()
}

pub fn append_observation(state: &mut AgentLoopState, observation: &str) {
    state.messages.push(Message {
        role: MessageRole::User,
        content: format!("Tool observation:\n{observation}\n\nContinue the task. If complete, answer with a concise final summary."),
        metadata: [("kind".to_string(), "tool_observation".to_string())]
            .into_iter()
            .collect(),
    });
}

pub fn append_tool_observation(
    state: &mut AgentLoopState,
    tool_call_id: ToolCallId,
    observation: &str,
) {
    state.messages.push(Message {
        role: MessageRole::Tool,
        content: observation.to_string(),
        metadata: [
            ("kind".to_string(), "tool_observation".to_string()),
            ("tool_call_id".to_string(), tool_call_id.0),
        ]
        .into_iter()
        .collect(),
    });
}

pub fn tool_invocation_from_request(
    task_id: &TaskId,
    request: &AgentToolRequest,
) -> ToolInvocation {
    ToolInvocation {
        id: request.call_id.clone(),
        task_id: task_id.clone(),
        tool_name: request.tool_name.clone(),
        input_json: request.input.clone(),
        proposed_by_model: "agent-loop".to_string(),
        metadata: [("agent_generated".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
    }
}

pub fn observation_from_tool_result(tool_name: &str, status: &str, output: &str) -> String {
    format!(
        "tool={tool_name}\nstatus={status}\noutput=\n{}",
        crate::tool_runtime::truncate_observation(output)
    )
}

pub fn observation_from_agent_tool_result(tool_name: &str, result: &ToolResult) -> String {
    const MODEL_OBSERVATION_LIMIT: usize = 6000;
    const TOOL_NAME_LIMIT: usize = 256;
    const SUMMARY_LIMIT: usize = 512;
    const FAILURE_CODE_LIMIT: usize = 256;
    const FACTS_LIMIT: usize = 1600;
    const NEXT_ACTION_LIMIT: usize = 768;
    const ARTIFACTS_LIMIT: usize = 1600;
    let Some(observation) = result
        .model_observation
        .as_ref()
        .filter(|observation| observation.schema == TOOL_OBSERVATION_V2_SCHEMA)
    else {
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
        return observation_from_tool_result(
            tool_name,
            crate::tool_runtime::tool_outcome_status_label(&result.status),
            &output,
        );
    };

    let effective_tool_name = if observation.tool_name.trim().is_empty() {
        tool_name
    } else {
        observation.tool_name.trim()
    };
    let (effective_tool_name, _) = truncate_observation_component(
        &single_line_observation_field(effective_tool_name),
        TOOL_NAME_LIMIT,
    );
    let (summary, _) = truncate_observation_component(
        &single_line_observation_field(&observation.summary),
        SUMMARY_LIMIT,
    );
    let mut rendered = format!(
        "tool={effective_tool_name}\nstatus={}\nschema={}\nsummary={}\nevidence_complete={}",
        crate::tool_runtime::tool_outcome_status_label(&result.status),
        TOOL_OBSERVATION_V2_SCHEMA,
        summary,
        observation.evidence_complete,
    );
    if let Some(failure) = &result.failure {
        let (failure_code, _) = truncate_observation_component(
            &single_line_observation_field(&failure.code),
            FAILURE_CODE_LIMIT,
        );
        rendered.push_str(&format!(
            "\nfailure_code={}\nretryable={}",
            failure_code, failure.retryable
        ));
    }
    if !observation.facts.is_empty() {
        let facts = serde_json::to_string(&observation.facts).unwrap_or_else(|_| "{}".to_string());
        let (facts, truncated) = truncate_observation_component(&facts, FACTS_LIMIT);
        rendered.push_str(if truncated {
            "\nfacts_excerpt="
        } else {
            "\nfacts="
        });
        rendered.push_str(&facts);
    }
    if let Some(next_action) = observation
        .next_action
        .as_deref()
        .map(str::trim)
        .filter(|next_action| !next_action.is_empty())
    {
        let (next_action, _) = truncate_observation_component(
            &single_line_observation_field(next_action),
            NEXT_ACTION_LIMIT,
        );
        rendered.push_str("\nnext_action=");
        rendered.push_str(&next_action);
    }
    if !result.artifacts.is_empty() {
        let artifacts = result
            .artifacts
            .iter()
            .map(|artifact| {
                serde_json::json!({
                    "path": artifact.path,
                    "mime_type": artifact.mime_type,
                    "title": artifact.title,
                })
            })
            .collect::<Vec<_>>();
        let artifacts = serde_json::to_string(&artifacts).unwrap_or_else(|_| "[]".to_string());
        let (artifacts, truncated) = truncate_observation_component(&artifacts, ARTIFACTS_LIMIT);
        rendered.push_str(if truncated {
            "\nartifacts_excerpt="
        } else {
            "\nartifacts="
        });
        rendered.push_str(&artifacts);
    }
    rendered.push_str("\noutput=\n");
    let evidence_budget = MODEL_OBSERVATION_LIMIT.saturating_sub(rendered.chars().count());
    rendered.push_str(&truncate_typed_observation_evidence(
        &observation.evidence,
        evidence_budget,
    ));
    rendered
}

fn single_line_observation_field(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_observation_component(value: &str, limit: usize) -> (String, bool) {
    const MARKER: &str = "...[truncated]...";
    let character_count = value.chars().count();
    if character_count <= limit {
        return (value.to_string(), false);
    }
    let marker_count = MARKER.chars().count();
    if limit <= marker_count {
        return (value.chars().take(limit).collect(), true);
    }
    let retained = limit - marker_count;
    let tail_count = retained / 4;
    let head_count = retained - tail_count;
    let head = value.chars().take(head_count).collect::<String>();
    let tail = value
        .chars()
        .skip(character_count - tail_count)
        .collect::<String>();
    (format!("{head}{MARKER}{tail}"), true)
}

fn truncate_typed_observation_evidence(evidence: &str, limit: usize) -> String {
    const MARKER: &str = "\n...[model evidence truncated]...\n";
    let character_count = evidence.chars().count();
    if character_count <= limit {
        return evidence.to_string();
    }
    let marker_count = MARKER.chars().count();
    if limit <= marker_count {
        return evidence.chars().take(limit).collect();
    }
    let retained = limit - marker_count;
    let tail_count = retained / 4;
    let head_count = retained.saturating_sub(tail_count);
    let head = evidence.chars().take(head_count).collect::<String>();
    let tail = evidence
        .chars()
        .skip(character_count.saturating_sub(tail_count))
        .collect::<String>();
    format!("{head}{MARKER}{tail}")
}

pub fn repeated_tool_failure_count(
    state: &AgentLoopState,
    tool_name: &str,
    input_json: &str,
) -> usize {
    let current = state
        .failed_tool_signatures
        .get(&tool_signature(tool_name, input_json))
        .copied()
        .unwrap_or_default();
    let legacy = state
        .failed_tool_signatures
        .get(&legacy_tool_signature(tool_name, input_json))
        .copied()
        .unwrap_or_default();
    current.saturating_add(legacy)
}

pub fn record_tool_outcome(
    state: &mut AgentLoopState,
    tool_name: &str,
    input_json: &str,
    status: &ToolOutcomeStatus,
) {
    record_tool_outcome_with_risk(state, tool_name, input_json, status, None);
}

pub fn record_tool_outcome_with_risk(
    state: &mut AgentLoopState,
    tool_name: &str,
    input_json: &str,
    status: &ToolOutcomeStatus,
    risk: Option<&ToolRisk>,
) {
    update_tool_failure_state(state, tool_name, input_json, status);
    let pending_before = state.task_contract.pending_interactions().len();
    state
        .task_contract
        .record_tool_outcome(tool_name, input_json, status, risk);
    update_verified_interactions(state, pending_before);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_tool_outcome_transition_with_risk(
    state: &mut AgentLoopState,
    tool_name: &str,
    input_json: &str,
    status: &ToolOutcomeStatus,
    risk: Option<&ToolRisk>,
    tool_spec: Option<&ToolSpec>,
    tool_evidence: Option<&ToolPostconditionEvidence>,
    observation: &str,
    lineage_scope: Option<&str>,
) -> Option<PostconditionVerificationReceipt> {
    update_tool_failure_state(state, tool_name, input_json, status);
    let pending_before = state.task_contract.pending_interactions().len();
    let steer_epoch = state.prepared_task_state.steer_epoch();
    let contract_epoch = state.prepared_task_state.contract_epoch();
    let receipt = state.task_contract.record_tool_outcome_transition(
        steer_epoch,
        contract_epoch,
        tool_name,
        input_json,
        status,
        risk,
        tool_spec,
        tool_evidence,
        None,
        observation,
        None,
        lineage_scope,
        true,
    );
    update_verified_interactions(state, pending_before);
    receipt
}

fn update_tool_failure_state(
    state: &mut AgentLoopState,
    tool_name: &str,
    input_json: &str,
    status: &ToolOutcomeStatus,
) {
    let signature = tool_signature(tool_name, input_json);
    let legacy_signature = legacy_tool_signature(tool_name, input_json);
    if matches!(
        status,
        ToolOutcomeStatus::Failed | ToolOutcomeStatus::Denied
    ) {
        let legacy_count = state
            .failed_tool_signatures
            .remove(&legacy_signature)
            .unwrap_or_default();
        let count = state.failed_tool_signatures.entry(signature).or_default();
        *count = count.saturating_add(legacy_count).saturating_add(1);
    } else {
        state.failed_tool_signatures.remove(&signature);
        state.failed_tool_signatures.remove(&legacy_signature);
    }
}

fn update_verified_interactions(state: &mut AgentLoopState, pending_before: usize) {
    let pending_after = state.task_contract.pending_interactions().len();
    if pending_after < pending_before {
        state.verified_interactions = state
            .verified_interactions
            .saturating_add(pending_before - pending_after);
    }
}

#[allow(clippy::too_many_arguments)]
fn record_persisted_tool_outcome_with_risk(
    state: &mut AgentLoopState,
    tool_name: &str,
    input_fingerprint: &str,
    redacted_effect_input: Option<&str>,
    postcondition_target_witness: Option<&task_contract::PostconditionTargetWitness>,
    allow_action_binding: bool,
    status: &ToolOutcomeStatus,
    risk: Option<&ToolRisk>,
    tool_spec: Option<&ToolSpec>,
    persisted_tool_evidence: Option<&tool_runtime::ReplayedToolPostconditionEvidence>,
    observation: &str,
) -> Option<PostconditionVerificationReceipt> {
    let signature = tool_failure_signature_from_input_fingerprint(input_fingerprint);
    if matches!(
        status,
        ToolOutcomeStatus::Failed | ToolOutcomeStatus::Denied
    ) {
        let count = state.failed_tool_signatures.entry(signature).or_default();
        *count = count.saturating_add(1);
    } else {
        state.failed_tool_signatures.remove(&signature);
    }

    let pending_before = state.task_contract.pending_interactions().len();
    let persisted_input = redacted_effect_input
        .map(str::to_string)
        .unwrap_or_else(|| persisted_tool_input_placeholder(input_fingerprint));
    let steer_epoch = state.prepared_task_state.steer_epoch();
    let contract_epoch = state.prepared_task_state.contract_epoch();
    let receipt = state.task_contract.record_tool_outcome_transition(
        steer_epoch,
        contract_epoch,
        tool_name,
        &persisted_input,
        status,
        risk,
        tool_spec,
        None,
        persisted_tool_evidence,
        observation,
        postcondition_target_witness,
        None,
        allow_action_binding,
    );
    update_verified_interactions(state, pending_before);
    receipt
}

fn persisted_tool_input_placeholder(input_fingerprint: &str) -> String {
    serde_json::json!({
        "permission_input_fingerprint": input_fingerprint,
    })
    .to_string()
}

fn rebuild_interaction_verification_state(state: &mut AgentLoopState) {
    let successful_tools = state
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .filter_map(|message| successful_tool_observation_name(&message.content))
        .map(str::to_string)
        .collect::<Vec<_>>();
    for tool_name in successful_tools {
        state.task_contract.record_tool_outcome(
            &tool_name,
            "{}",
            &ToolOutcomeStatus::Succeeded,
            None,
        );
    }
}

fn successful_tool_observation_name(content: &str) -> Option<&str> {
    let mut tool_name = None;
    let mut succeeded = false;
    for line in content.lines().take(4) {
        if let Some(value) = line.strip_prefix("tool=") {
            tool_name = Some(value.trim());
        } else if let Some(value) = line.strip_prefix("status=") {
            succeeded = value.trim() == "succeeded";
        }
    }
    succeeded
        .then_some(tool_name?)
        .filter(|name| !name.is_empty())
}

pub fn interaction_completion_verification_instruction(
    state: &mut AgentLoopState,
    tools: &[ToolSpec],
) -> Option<String> {
    state
        .task_contract
        .completion_instruction(false, tools)
        .ok()
        .flatten()
}

pub fn completion_verification_instruction(
    state: &mut AgentLoopState,
    verification_required: bool,
    tools: &[ToolSpec],
) -> Option<String> {
    state
        .task_contract
        .completion_instruction(verification_required, tools)
        .ok()
        .flatten()
}

fn tool_signature(tool_name: &str, input_json: &str) -> String {
    tool_failure_signature_from_input_fingerprint(&tool_input_fingerprint(tool_name, input_json))
}

fn tool_failure_signature_from_input_fingerprint(input_fingerprint: &str) -> String {
    let input_fingerprint = input_fingerprint.trim();
    let canonical_fingerprint = if input_fingerprint.len() == 64
        && input_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        input_fingerprint.to_ascii_lowercase()
    } else {
        tool_input_fingerprint("persisted-tool-input-fingerprint", input_fingerprint)
    };
    format!("{TOOL_FAILURE_SIGNATURE_SCHEMA}:{canonical_fingerprint}")
}

fn legacy_tool_signature(tool_name: &str, input_json: &str) -> String {
    let canonical = serde_json::from_str::<serde_json::Value>(input_json)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| input_json.trim().to_string());
    format!("{tool_name}\n{canonical}")
}

fn normalized_failed_tool_signatures(
    signatures: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    let mut normalized = BTreeMap::<String, usize>::new();
    for (signature, count) in signatures {
        if *count == 0 {
            continue;
        }
        let normalized_signature = if let Some(fingerprint) =
            signature.strip_prefix(&format!("{TOOL_FAILURE_SIGNATURE_SCHEMA}:"))
        {
            tool_failure_signature_from_input_fingerprint(fingerprint)
        } else if let Some((tool_name, input_json)) = signature.split_once('\n') {
            tool_signature(tool_name, input_json)
        } else {
            tool_failure_signature_from_input_fingerprint(&tool_input_fingerprint(
                "legacy-tool-failure-signature",
                signature,
            ))
        };
        let normalized_count = normalized.entry(normalized_signature).or_default();
        *normalized_count = normalized_count.saturating_add(*count);
    }
    normalized
}

pub fn agent_system_prompt(tools: &[ToolSpec]) -> String {
    agent_system_prompt_with_override(tools, None)
}

fn original_tool_name(model_name: &str, tools: &[ToolSpec]) -> String {
    tools
        .iter()
        .find(|tool| tool.name == model_name || tool_function_name(&tool.name) == model_name)
        .map(|tool| tool.name.clone())
        .unwrap_or_else(|| model_name.replace('_', "."))
}

#[cfg(test)]
mod tests;
