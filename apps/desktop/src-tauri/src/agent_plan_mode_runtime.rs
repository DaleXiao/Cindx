//! Plan mode (plan-then-confirm) for high/xhigh effort runs.
//!
//! Plan mode is an interaction feature, not a planning system: when the user
//! explicitly asks for a plan first on a High/Xhigh run, the run drafts a plan
//! with a bounded read-only tool loop (the same whitelist and Worker-stage
//! budget charging as subagent delegation), then pauses for an explicit user
//! decision before any preparation or execution. The deterministic effort-tier
//! `EffortRunPlan` remains the sole scheduling authority; a confirmed plan is
//! injected into the execution context as a protected, provenance-stamped
//! guidance source (the same pipeline pattern as project instructions) and
//! never writes a scheduling key.

use crate::agent_query_commands::agent_run_should_stop;
use agent_core::{
    AgentPolicy, Event, EventKind, Message, MessageRole, Metadata, ModelRole, TaskId, ToolSpec,
};
use agent_runtime::{
    subagent_tool_allowed, AgentRunControl, RunStageClass, CONTEXT_SOURCE_SCHEMA,
    SUBAGENT_MAX_STEPS,
};
use model_provider::{ModelCallMode, ModelRequest, StreamingModelProvider};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tools::ToolRegistry;

pub(crate) const PLAN_MODE_SCHEMA: &str = "cindx.plan-mode.v1";
pub(crate) const CONFIRMED_PLAN_MESSAGE_KIND: &str = "confirmed_plan";
pub(crate) const PLAN_MODE_REQUESTED_KEY: &str = "plan_mode_requested";
pub(crate) const PLAN_PROPOSED_SUMMARY: &str = "Agent plan proposed";
pub(crate) const PLAN_RESOLVED_SUMMARY: &str = "Agent plan confirmation resolved";
pub(crate) const PLAN_RECOVERY_REASON: &str = "waiting_for_plan_confirmation";
pub(crate) const PLAN_MAX_CHARS: usize = 8_000;
const PLAN_STAGE_LABEL: &str = "plan";

/// Plan confirmation decisions recorded on the `plan_resolved` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanConfirmationDecision {
    Approved,
    Discarded,
    Cancelled,
}

impl PlanConfirmationDecision {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Discarded => "discarded",
            Self::Cancelled => "cancelled",
        }
    }
}

pub(crate) fn parse_plan_confirmation_decision(
    value: &str,
) -> Result<PlanConfirmationDecision, String> {
    match value {
        "approve" | "approved" => Ok(PlanConfirmationDecision::Approved),
        "discard" | "discarded" => Ok(PlanConfirmationDecision::Discarded),
        "cancel" | "cancelled" => Ok(PlanConfirmationDecision::Cancelled),
        other => Err(format!("unknown plan confirmation decision: {other}")),
    }
}

/// Who resolved the plan confirmation: the user clicking a card action, or
/// the card's idle auto-approve timer. The value is recorded on the
/// `plan_resolved` event so an audit can always tell a timeout approval from
/// an explicit user decision.
pub(crate) const PLAN_RESOLVED_BY_USER: &str = "local-user";
pub(crate) const PLAN_RESOLVED_BY_AUTO_TIMEOUT: &str = "auto-timeout";

pub(crate) fn parse_plan_resolved_by(value: &str) -> Result<&'static str, String> {
    match value {
        "" | "local-user" => Ok(PLAN_RESOLVED_BY_USER),
        "auto-timeout" => Ok(PLAN_RESOLVED_BY_AUTO_TIMEOUT),
        other => Err(format!("unknown plan resolution source: {other}")),
    }
}

/// The Composer entry exists only for High/Xhigh; Fast/Default never expose or
/// honor plan mode.
pub(crate) fn plan_mode_available_for_effort(effort: AgentPolicy) -> bool {
    matches!(effort, AgentPolicy::High | AgentPolicy::Xhigh)
}

/// A plan-mode request is honored only when the user explicitly asked and the
/// effort tier supports it; a stale request flag on a Fast/Default run is
/// dropped instead of silently activating the gate.
pub(crate) fn plan_mode_gate_active(requested: bool, effort: AgentPolicy) -> bool {
    requested && plan_mode_available_for_effort(effort)
}

/// Deterministic complexity gate for the plan-first toggle. Trivial requests
/// (greetings, short questions, chit-chat) skip the plan phase even when the
/// toggle is on, while anything that reads like engineering work keeps it; an
/// explicit ask for a plan always wins. Pure and model-free, matching the
/// deterministic planning philosophy: the toggle states intent, this gate
/// states whether the request warrants a plan.
pub(crate) fn plan_first_warranted(prompt: &str) -> bool {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return false;
    }
    if explicit_plan_request(trimmed) {
        return true;
    }
    if trimmed.chars().count() > 160 {
        return true;
    }
    if contains_path_or_file_reference(trimmed) {
        return true;
    }
    contains_action_verb(trimmed)
}

fn explicit_plan_request(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    lower.contains("计划")
        || lower.contains("规划")
        || lower.starts_with("plan")
        || lower.contains(" plan ")
        || lower.contains("plan:")
        || lower.contains("plan first")
        || lower.contains("a plan")
        || lower.contains("the plan")
}

fn contains_path_or_file_reference(prompt: &str) -> bool {
    if prompt.contains('`') {
        return true;
    }
    const FILE_EXTENSIONS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".md", ".json", ".toml", ".yaml", ".yml",
        ".css", ".html", ".sh", ".mjs", ".cjs", ".go", ".swift", ".kt",
    ];
    if FILE_EXTENSIONS
        .iter()
        .any(|extension| prompt.contains(extension))
    {
        return true;
    }
    // A slash between word characters reads as a path (src/foo, apps/desktop).
    prompt.as_bytes().windows(3).any(|window| {
        window[1] == b'/' && window[0].is_ascii_alphanumeric() && window[2].is_ascii_alphanumeric()
    })
}

fn contains_action_verb(prompt: &str) -> bool {
    const ENGLISH_VERBS: &[&str] = &[
        "add ",
        "adds ",
        "create ",
        "creates ",
        "build ",
        "builds ",
        "implement ",
        "implements ",
        "refactor",
        "fix ",
        "fixes ",
        "write ",
        "writes ",
        "update ",
        "updates ",
        "change ",
        "changes ",
        "delete ",
        "removes ",
        "remove ",
        "rename",
        "move ",
        "moves ",
        "migrate",
        "integrate",
        "set up",
        "setup",
        "deploy",
        "configure",
        "optimize",
        "improve",
        "generate",
        "make ",
        "install ",
        "upgrade",
        "replace",
        "extract",
        "split",
        "merge ",
    ];
    const CHINESE_VERBS: &[&str] = &[
        "加", "写", "改", "修", "建", "删", "重构", "实现", "添加", "创建", "修复", "更新", "删除",
        "移动", "迁移", "配置", "优化", "生成", "搭建", "接入", "替换", "拆分", "合并", "统一",
        "调整", "支持", "增加", "做一", "处理",
    ];
    let lower = prompt.to_lowercase();
    ENGLISH_VERBS.iter().any(|verb| lower.contains(verb))
        || CHINESE_VERBS.iter().any(|verb| prompt.contains(verb))
}

pub(crate) fn plan_mode_requested_in_context(run_context: &Metadata) -> bool {
    run_context.get(PLAN_MODE_REQUESTED_KEY).map(String::as_str) == Some("true")
}

/// Whether the active run was started with plan mode requested. A resumed
/// attempt re-derives this from its durable start event so the confirmed-plan
/// injection survives suspension and restart.
pub(crate) fn plan_mode_requested_in_events(active_events: &[Event]) -> bool {
    active_events
        .iter()
        .find(|event| {
            agent_application::AgentRunEvent::from_event(event)
                .is_some_and(agent_application::AgentRunEvent::is_start)
        })
        .and_then(|event| event.metadata.get(PLAN_MODE_REQUESTED_KEY))
        .map(String::as_str)
        == Some("true")
}

/// The read-only tool surface of the plan phase, resolved through the same
/// deterministic whitelist the subagent loop uses so plan drafting can never
/// produce a side effect.
pub(crate) fn plan_phase_tool_specs(registry: &ToolRegistry) -> Vec<ToolSpec> {
    registry
        .specs()
        .into_iter()
        .filter(|spec| subagent_tool_allowed(&spec.name))
        .collect()
}

pub(crate) fn plan_mode_system_prompt() -> &'static str {
    "You draft execution plans for an agent run. You operate read-only: you may inspect the \
     workspace with the provided read-only tools, but you must never attempt to change anything. \
     Answer with the plan only, in this exact shape:\n\
     Objective: <one sentence>\n\
     Steps:\n\
     1. <imperative step title> - <one short line of detail> (files: path/a, path/b)\n\
     2. ...\n\
     Verification: <how the result will be checked>\n\
     Keep it to at most 7 steps; omit the files note for steps that touch no files. Do not nest \
     sub-plans, do not execute the plan, and do not ask questions; if information is missing, \
     state the assumption inside the step."
}

pub(crate) fn build_plan_phase_user_prompt(objective: &str) -> String {
    format!(
        "Draft an execution plan for the following request. Inspect the workspace with the \
         read-only tools when that improves the plan, then answer with the plan only.\n\n\
         Request:\n{objective}"
    )
}

pub(crate) fn plan_digest(plan_markdown: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PLAN_MODE_SCHEMA.as_bytes());
    hasher.update([0u8]);
    hasher.update(plan_markdown.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Outcome of the read-only plan-drafting loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanPhaseOutcome {
    /// A bounded plan was drafted and is ready for user confirmation.
    Proposed(String),
    /// The plan could not be drafted (provider error, exhausted Worker-stage
    /// budget, or an empty answer); the run proceeds without a plan.
    Unavailable(String),
    /// The run was cancelled or stopped while drafting.
    Stopped,
}

/// Run the bounded read-only plan-drafting loop. Every model call is charged
/// to the parent run's Worker stage budget, the loop honors run cancellation,
/// and only whitelisted read-only tools may execute — an effectful or
/// out-of-policy call becomes a denied observation instead of an execution.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_plan_phase(
    actor_provider: &dyn StreamingModelProvider,
    objective: &str,
    cancellation: &Arc<AgentRunControl>,
    registry: &ToolRegistry,
    plan_tools: &[ToolSpec],
    task_id: &TaskId,
    on_tool_call: &mut dyn FnMut(&str, &str, &str),
) -> PlanPhaseOutcome {
    let mut messages = vec![
        Message {
            role: MessageRole::System,
            content: plan_mode_system_prompt().to_string(),
            metadata: Metadata::new(),
        },
        Message {
            role: MessageRole::User,
            content: build_plan_phase_user_prompt(objective),
            metadata: Metadata::new(),
        },
    ];
    for _step in 0..SUBAGENT_MAX_STEPS {
        if agent_run_should_stop(cancellation) {
            return PlanPhaseOutcome::Stopped;
        }
        // Charge the model call to the run's Worker stage budget; exhausting it
        // abandons plan drafting without stopping the run itself.
        if cancellation
            .begin_stage_model_call(PLAN_STAGE_LABEL, RunStageClass::Worker)
            .is_err()
        {
            return PlanPhaseOutcome::Unavailable(
                "plan stage budget exhausted before a plan was drafted".to_string(),
            );
        }
        let request = ModelRequest {
            role: ModelRole::Executor,
            messages: messages.clone(),
            tools: plan_tools.to_vec(),
            mode: ModelCallMode::NonStreaming,
            metadata: Metadata::new(),
        };
        let mut should_cancel = || agent_run_should_stop(cancellation);
        // Reserve a physical attempt on the unified ledger (audit P1-01): the
        // logical stage call above bounded plan drafting, but the physical
        // tokens/attempts were never counted against the run budget.
        // Wire activity marks run progress (throttled 1s), same contract as
        // the foreground turn and subagent loops.
        let mut last_activity_mark = std::time::Instant::now();
        let mut on_activity = || {
            if last_activity_mark.elapsed() >= std::time::Duration::from_secs(1) {
                last_activity_mark = std::time::Instant::now();
                cancellation.note_wire_activity("plan", "provider wire activity");
            }
        };
        let outcome = crate::model_resource_runtime::controlled_aux_model_call(
            cancellation,
            "plan",
            &request,
            RunStageClass::Worker,
            || {
                actor_provider.complete_streaming_cancellable_with_activity(
                    request.clone(),
                    &mut |_| {},
                    &mut on_activity,
                    &mut should_cancel,
                )
            },
        );
        cancellation.finish_model_call();
        let response = match outcome {
            crate::model_resource_runtime::AuxModelCall::Response(response) => response,
            crate::model_resource_runtime::AuxModelCall::BudgetExhausted => {
                return PlanPhaseOutcome::Unavailable(
                    "plan physical resource budget exhausted before a plan was drafted".to_string(),
                );
            }
            _ => {
                return PlanPhaseOutcome::Unavailable(
                    "plan drafting model call failed".to_string(),
                );
            }
        };
        if response.tool_calls.is_empty() {
            let plan = response.message.content.trim().to_string();
            if plan.is_empty() {
                return PlanPhaseOutcome::Unavailable(
                    "plan drafting returned an empty plan".to_string(),
                );
            }
            return PlanPhaseOutcome::Proposed(truncate_plan_to_budget(&plan));
        }
        // Record the assistant turn (with its raw tool calls) so the next
        // request payload is well-formed, then execute each read-only call.
        let mut assistant_metadata = Metadata::new();
        if let Some(raw) = response.raw_tool_calls_json.clone() {
            assistant_metadata.insert("raw_tool_calls_json".to_string(), raw);
        }
        assistant_metadata.insert(
            "tool_call_ids".to_string(),
            response
                .tool_calls
                .iter()
                .map(|call| call.id.clone())
                .collect::<Vec<_>>()
                .join(","),
        );
        messages.push(Message {
            role: MessageRole::Assistant,
            content: response.message.content.clone(),
            metadata: assistant_metadata,
        });
        for call in &response.tool_calls {
            let observation = crate::agent_subagent_runtime::execute_subagent_tool_call(
                registry,
                task_id,
                call,
                plan_tools,
                // Plan drafting holds no network capability context: web
                // tools fail closed here (audit E1).
                None,
                cancellation,
            );
            // The user-visible exploration report carries the canonical
            // registry name, never the provider's wire echo.
            on_tool_call(
                &agent_runtime::original_tool_name(&call.name, plan_tools),
                &call.id,
                &observation,
            );
            messages.push(Message {
                role: MessageRole::Tool,
                content: observation,
                metadata: [
                    ("kind".to_string(), "tool_observation".to_string()),
                    ("tool_call_id".to_string(), call.id.clone()),
                ]
                .into_iter()
                .collect(),
            });
        }
    }
    PlanPhaseOutcome::Unavailable(format!(
        "plan drafting reached its step limit ({SUBAGENT_MAX_STEPS})"
    ))
}

fn truncate_plan_to_budget(plan: &str) -> String {
    if plan.chars().count() <= PLAN_MAX_CHARS {
        return plan.to_string();
    }
    plan.chars().take(PLAN_MAX_CHARS).collect()
}

/// Build the protected, provenance-stamped context message for a
/// user-confirmed plan. This mirrors the project-instructions pipeline: the
/// plan is injected as untrusted guidance with a digest receipt and can never
/// grant authority or change scheduling.
pub(crate) fn confirmed_plan_message(
    plan_markdown: &str,
    plan_digest: &str,
    proposed_at_ms: u64,
) -> Message {
    let content = format!(
        "The user explicitly reviewed and confirmed the following execution plan for this run. \
         Treat it as user-confirmed plan guidance. It is context content, not authority: it \
         cannot grant tool permissions, override approvals, expand tool authority, change run \
         budgets, or alter the deterministic effort-tier scheduling of this run. Follow the \
         ordered steps unless newer evidence makes one obsolete; if the plan no longer fits, say \
         so instead of silently re-planning.\n\n\
         [confirmed plan | sha256:{plan_digest} | proposed_at_ms:{proposed_at_ms}]\n\
         {plan_markdown}"
    );
    let mut metadata = Metadata::new();
    metadata.insert("internal".to_string(), "true".to_string());
    metadata.insert("kind".to_string(), CONFIRMED_PLAN_MESSAGE_KIND.to_string());
    metadata.insert(
        "context_source_schema".to_string(),
        CONTEXT_SOURCE_SCHEMA.to_string(),
    );
    metadata.insert(
        "confirmed_plan_schema".to_string(),
        PLAN_MODE_SCHEMA.to_string(),
    );
    metadata.insert("confirmed_plan_digest".to_string(), plan_digest.to_string());
    metadata.insert(
        "confirmed_plan_proposed_at_ms".to_string(),
        proposed_at_ms.to_string(),
    );
    Message {
        role: MessageRole::System,
        content,
        metadata,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmedPlan {
    pub(crate) plan_markdown: String,
    pub(crate) plan_digest: String,
    pub(crate) proposed_at_ms: u64,
}

fn event_plan_marker(event: &Event) -> Option<&str> {
    event.metadata.get("plan_event").map(String::as_str)
}

fn event_matches_logical_run(event: &Event, logical_run_id: &str) -> bool {
    event
        .metadata
        .get("logical_agent_run_id")
        .map(String::as_str)
        == Some(logical_run_id)
}

/// Load the user-confirmed plan for one logical run from durable run events.
/// Only an `approved` resolution injects; a discarded or cancelled decision,
/// a missing proposal, or a different logical run all yield no plan.
pub(crate) fn confirmed_plan_in_events(
    events: &[Event],
    logical_run_id: &str,
) -> Option<ConfirmedPlan> {
    let resolution = events.iter().rev().find(|event| {
        event_plan_marker(event) == Some("resolved")
            && event_matches_logical_run(event, logical_run_id)
    })?;
    if resolution.metadata.get("plan_decision").map(String::as_str) != Some("approved") {
        return None;
    }
    let digest = resolution.metadata.get("plan_digest")?.as_str();
    let proposal = events.iter().find(|event| {
        event_plan_marker(event) == Some("proposed")
            && event_matches_logical_run(event, logical_run_id)
            && event.metadata.get("plan_digest").map(String::as_str) == Some(digest)
    })?;
    let plan_markdown = proposal.metadata.get("plan_markdown")?.clone();
    if plan_markdown.trim().is_empty() {
        return None;
    }
    let proposed_at_ms = proposal
        .metadata
        .get("plan_proposed_at_ms")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(proposal.timestamp_ms);
    Some(ConfirmedPlan {
        plan_markdown,
        plan_digest: digest.to_string(),
        proposed_at_ms,
    })
}

/// Inject the user-confirmed plan of the current logical run into the prepared
/// execution context, following the project-instructions pipeline pattern: a
/// protected system message plus provenance receipt keys on the run context.
/// This writes only `confirmed_plan_*` provenance keys and never touches a
/// scheduling key.
pub(crate) fn append_confirmed_plan_context(
    events: &[Event],
    run_context: &mut Metadata,
    history: &mut Vec<Message>,
) {
    if !plan_mode_requested_in_context(run_context) {
        return;
    }
    let Some(logical_run_id) = run_context.get("logical_agent_run_id").cloned() else {
        return;
    };
    let Some(confirmed) = confirmed_plan_in_events(events, &logical_run_id) else {
        return;
    };
    run_context.insert(
        "confirmed_plan_schema".to_string(),
        PLAN_MODE_SCHEMA.to_string(),
    );
    run_context.insert(
        "confirmed_plan_digest".to_string(),
        confirmed.plan_digest.clone(),
    );
    history.push(confirmed_plan_message(
        &confirmed.plan_markdown,
        &confirmed.plan_digest,
        confirmed.proposed_at_ms,
    ));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingPlanConfirmation {
    pub(crate) plan_markdown: String,
    pub(crate) plan_digest: String,
    pub(crate) proposed_at_ms: u64,
}

/// The active run is parked at the plan-confirmation gate: the latest
/// lifecycle event is the nonterminal pause and the run holds a plan proposal.
pub(crate) fn plan_mode_pause_present(active_events: &[Event]) -> bool {
    let latest_lifecycle = active_events
        .iter()
        .rev()
        .find_map(agent_application::AgentRunEvent::from_event);
    if latest_lifecycle != Some(agent_application::AgentRunEvent::Paused) {
        return false;
    }
    active_events
        .iter()
        .any(|event| event_plan_marker(event) == Some("proposed"))
}

/// Project the plan awaiting user confirmation for the active run, if any:
/// the latest proposal whose digest has no matching resolution yet, while the
/// run is paused at the plan gate.
pub(crate) fn pending_plan_confirmation(
    active_events: &[Event],
) -> Option<PendingPlanConfirmation> {
    if !plan_mode_pause_present(active_events) {
        return None;
    }
    let proposal = active_events
        .iter()
        .rev()
        .find(|event| event_plan_marker(event) == Some("proposed"))?;
    let digest = proposal.metadata.get("plan_digest")?.as_str();
    let resolved = active_events.iter().any(|event| {
        event_plan_marker(event) == Some("resolved")
            && event.metadata.get("plan_digest").map(String::as_str) == Some(digest)
    });
    if resolved {
        return None;
    }
    let plan_markdown = proposal.metadata.get("plan_markdown")?.clone();
    if plan_markdown.trim().is_empty() {
        return None;
    }
    Some(PendingPlanConfirmation {
        plan_markdown,
        plan_digest: digest.to_string(),
        proposed_at_ms: proposal
            .metadata
            .get("plan_proposed_at_ms")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(proposal.timestamp_ms),
    })
}

/// Metadata carried by the `plan_proposed` event: the plan content and its
/// content-bound digest. Run identity and session context ride along through
/// `metadata_with_context` at the call site.
pub(crate) fn plan_proposed_event_metadata(plan_markdown: &str) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert("plan_event".to_string(), "proposed".to_string());
    metadata.insert("plan_schema".to_string(), PLAN_MODE_SCHEMA.to_string());
    metadata.insert("plan_digest".to_string(), plan_digest(plan_markdown));
    metadata.insert("plan_markdown".to_string(), plan_markdown.to_string());
    metadata
}

/// Metadata carried by the `plan_resolved` event: the user's decision bound to
/// the exact plan digest it answered, plus the resolution source
/// (`local-user` or `auto-timeout`) so timeout approvals stay auditable.
pub(crate) fn plan_resolved_event_metadata(
    decision: PlanConfirmationDecision,
    plan_digest: &str,
    resolved_by: &str,
) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert("plan_event".to_string(), "resolved".to_string());
    metadata.insert("plan_schema".to_string(), PLAN_MODE_SCHEMA.to_string());
    metadata.insert("plan_digest".to_string(), plan_digest.to_string());
    metadata.insert("plan_decision".to_string(), decision.label().to_string());
    metadata.insert("plan_resolved_by".to_string(), resolved_by.to_string());
    metadata
}

pub(crate) fn plan_event_kind() -> EventKind {
    EventKind::TaskStatusChanged
}

/// The model slot used to draft the plan is the tier-selected primary model —
/// the same model that would execute the run.
pub(crate) fn plan_phase_model(
    config: &crate::configuration_models::ProviderConfig,
    effort: AgentPolicy,
) -> String {
    crate::agent_run_engine::effort_tier_model(config, effort.label())
}

/// Build the provider used for the read-only plan-drafting loop. The call
/// timeout comes from the Worker stage allowance, matching the budget class
/// the calls are charged to.
pub(crate) fn plan_phase_provider(
    config: &crate::configuration_models::ProviderConfig,
    agent_model: &str,
    cancellation: &AgentRunControl,
) -> model_provider::OpenAiCompatibleProvider {
    model_provider::OpenAiCompatibleProvider::new(model_provider::OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: agent_model.to_string(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: cancellation.stage_model_call_timeout_seconds(RunStageClass::Worker),
    })
}

/// Outcome of the plan-mode gate at run start.
pub(crate) enum PlanModeGateOutcome {
    /// A plan was drafted and durably recorded; the run is paused until the
    /// user approves, discards, or cancels it.
    AwaitingConfirmation(Box<crate::view_models::AgentState>),
    /// No plan gate applies (or drafting was unavailable); the run proceeds
    /// with ordinary preparation and execution.
    Proceed,
    /// The run was stopped or cancelled while drafting.
    Stopped,
}

/// Run the plan-then-confirm gate for a High/Xhigh run whose user explicitly
/// enabled plan mode: draft a plan with the bounded read-only loop, persist the
/// proposal, and pause the run (the existing nonterminal pause + recovery
/// envelope mechanism) until the user resolves the confirmation. Plan drafting
/// model calls are charged to the run's Worker stage budget; a drafting failure
/// is recorded and the run proceeds without a plan.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_plan_mode_gate(
    state: &crate::app_state::AppState,
    config: &crate::configuration_models::ProviderConfig,
    workspace_root: &Path,
    task_id: &TaskId,
    run_context: &Metadata,
    prompt: &str,
    effort: AgentPolicy,
    cancellation: &Arc<AgentRunControl>,
) -> Result<PlanModeGateOutcome, String> {
    if !plan_mode_requested_in_context(run_context) || !plan_mode_available_for_effort(effort) {
        return Ok(PlanModeGateOutcome::Proceed);
    }
    if !plan_first_warranted(prompt) {
        // Adaptive plan-first: the toggle states intent, but a trivial request
        // (greeting, short question) gains nothing from a plan phase. The skip
        // is deterministic, recorded for observability, and never blocks the run.
        crate::agent_query_commands::append_agent_progress_event(
            state,
            task_id,
            run_context,
            "Plan phase skipped: the request does not warrant a plan",
        )?;
        return Ok(PlanModeGateOutcome::Proceed);
    }
    if !cancellation.begin_preparation() {
        return Ok(PlanModeGateOutcome::Stopped);
    }
    let session_id = run_context.get("session_id").map(String::as_str);
    let agent_model = plan_phase_model(config, effort);
    let provider = plan_phase_provider(config, &agent_model, cancellation);
    let registry = crate::persistence_runtime::tool_registry_for_state(state, workspace_root)?;
    let plan_tools = plan_phase_tool_specs(&registry);
    let objective = crate::runtime_values::truncate_for_collaboration(prompt, 6_000);
    crate::agent_query_commands::append_agent_progress_event(
        state,
        task_id,
        run_context,
        "Drafting a plan for confirmation",
    )?;
    let outcome = run_plan_phase(
        &provider,
        &objective,
        cancellation,
        &registry,
        &plan_tools,
        task_id,
        &mut |tool: &str, call_id: &str, detail: &str| {
            // Surface each read-only exploration call as start/finish/observation
            // events so the thread shows a live "Agent actions" chain and the run
            // reads as working while the plan is drafted.
            if let Ok(mut store) = state.store.lock() {
                let base = |status: &str| {
                    let mut metadata = agent_core::Metadata::new();
                    metadata.insert("kind".to_string(), "tool_observation".to_string());
                    metadata.insert("tool".to_string(), tool.to_string());
                    metadata.insert("tool_call_id".to_string(), call_id.to_string());
                    metadata.insert("status".to_string(), status.to_string());
                    crate::project_session_persistence::metadata_with_context(metadata, run_context)
                };
                let _ = crate::event_persistence::append_event(
                    &mut store,
                    task_id,
                    agent_core::EventKind::ToolCallStarted,
                    format!("Plan exploration: {tool}"),
                    base("running"),
                );
                let _ = crate::event_persistence::append_message_event_with_metadata(
                    &mut store,
                    task_id,
                    MessageRole::Tool,
                    detail,
                    base("done"),
                );
                let _ = crate::event_persistence::append_event(
                    &mut store,
                    task_id,
                    agent_core::EventKind::ToolCallFinished,
                    format!("Plan exploration done: {tool}"),
                    base("done"),
                );
            }
        },
    );
    let plan = match outcome {
        PlanPhaseOutcome::Proposed(plan) => plan,
        PlanPhaseOutcome::Stopped => return Ok(PlanModeGateOutcome::Stopped),
        PlanPhaseOutcome::Unavailable(reason) => {
            if agent_run_should_stop(cancellation) {
                return Ok(PlanModeGateOutcome::Stopped);
            }
            crate::agent_query_commands::append_agent_progress_event(
                state,
                task_id,
                run_context,
                &format!("Plan mode unavailable ({reason}); continuing without a plan"),
            )?;
            return Ok(PlanModeGateOutcome::Proceed);
        }
    };
    let proposal_metadata = plan_proposed_event_metadata(&plan);
    let plan_digest = proposal_metadata
        .get("plan_digest")
        .cloned()
        .unwrap_or_default();
    let resource_snapshot = cancellation.resource_usage();
    let steer_epoch = run_context
        .get("steer_epoch")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let committed = cancellation.commit_preparation_checkpoint_with(steer_epoch, || {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .with_immediate_transaction(|store| {
                crate::event_persistence::append_event(
                    store,
                    task_id,
                    plan_event_kind(),
                    PLAN_PROPOSED_SUMMARY,
                    crate::project_session_persistence::metadata_with_context(
                        proposal_metadata.clone(),
                        run_context,
                    ),
                )?;
                let events = crate::agent_read_model::agent_events_for_session(
                    store,
                    &crate::runtime_values::phase16_task_id(),
                    session_id,
                )?;
                let active_events =
                    crate::agent_read_model::active_agent_events_for_session(&events, session_id);
                let recovery_metadata =
                    crate::agent_recovery_service::agent_recovery_metadata_with_task_state(
                        &active_events,
                        run_context,
                        agent_application::AgentRecoveryState::Paused,
                        agent_application::AgentRecoveryReason::Unknown(
                            PLAN_RECOVERY_REASON.to_string(),
                        ),
                        [
                            ("plan_digest".to_string(), plan_digest.clone()),
                            ("plan_mode".to_string(), "awaiting_confirmation".to_string()),
                        ]
                        .into_iter()
                        .collect(),
                        None,
                        Some(&resource_snapshot),
                    )
                    .map_err(agent_storage::StorageError::new)?;
                crate::event_persistence::append_event(
                    store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Agent task paused",
                    recovery_metadata,
                )?;
                crate::agent_read_model::agent_state_for_session(store, None, session_id)
            })
            .map_err(|error| error.to_string())
    })?;
    match committed {
        agent_runtime::RunPreparationCheckpoint::Committed(state) => {
            Ok(PlanModeGateOutcome::AwaitingConfirmation(Box::new(state)))
        }
        // A cancellation or a steer that won first abandons the proposal before
        // it is durably recorded; the run follows the winner instead.
        agent_runtime::RunPreparationCheckpoint::Stopped(_) => Ok(PlanModeGateOutcome::Stopped),
        agent_runtime::RunPreparationCheckpoint::RestartAfterSteer => {
            Ok(PlanModeGateOutcome::Proceed)
        }
    }
}

#[cfg(test)]
#[path = "agent_plan_mode_runtime_tests.rs"]
mod tests;
