//! Borrowed from opencode `tool/task.ts`, pi `subagent`, and deepseek-harness
//! `subagent`/`goal-round-driver`: a subagent is an isolated child run that sees
//! the delegated task plus a bounded seed of the parent's completed rounds, uses
//! a restricted read-only tool set, and returns its final text to the parent.
//! This module holds the deterministic policy; the child execution is wired by
//! the desktop loop.

use agent_core::{Message, MessageRole, Metadata};
use sha2::{Digest, Sha256};

// Raised from 8 so delegated work can run deeper multi-step loops.
pub const SUBAGENT_MAX_STEPS: usize = 12;

/// Per-child tool-call cap (P1-01 resource governance): bounds how many tool
/// calls one subagent child may execute so parallel children cannot amplify tool
/// calls unbounded. Exhaustion stops the child, not the parent run.
pub const SUBAGENT_MAX_TOOL_CALLS: usize = 48;

/// Maximum parent messages a subagent context fork may seed into the child.
// Raised from 32 so a forked subagent inherits more of the parent's context.
pub const SUBAGENT_CONTEXT_FORK_MAX_MESSAGES: usize = 48;

/// Tools a subagent may use: read-only discovery plus the controlled network
/// capability (web.search/web.fetch, gated by
/// `ToolRegistry::worker_network_read_tool`). Effectful, delegation, and
/// working-memory tools are denied (mirrors opencode deriveSubagentSessionPermission).
pub const SUBAGENT_ALLOWED_TOOLS: &[&str] = &[
    "file.read",
    "file.read_many",
    "file.list",
    "file.search",
    "file.glob",
    "web.search",
    "web.fetch",
];

/// The only write tools a patch-capable ("write") subagent may ever receive.
/// Both are Write-risk, permission-gated workspace patch tools; the parent
/// run's permission path approves every single call (no session grant reuse).
pub const SUBAGENT_PATCH_TOOLS: &[&str] = &["file.patch", "file.patch_batch"];

pub fn subagent_system_prompt() -> &'static str {
    "You are a subagent handling one delegated task in an isolated context. Work it to completion using only the read-only discovery tools provided, then answer concisely with the concrete result (findings, file paths, or a short summary). When you report findings from files, cite each one as path:line so the parent can reference the exact location. Do not ask the user questions; if blocked, say what is blocked."
}

/// System prompt for a write subagent (delegated with `allow_patches`): the
/// read-only contract plus the patch discipline. Every patch call pauses for
/// an explicit user approval; a denial is an observation to work around or
/// report, never a reason to retry the identical patch.
pub fn subagent_write_system_prompt() -> &'static str {
    "You are a subagent handling one delegated task in an isolated context. Work it to completion using only the provided tools, then answer concisely with the concrete result (what changed, file paths, or a short summary). You may patch existing workspace files with file.patch or file.patch_batch: always read a file first and base the patch on the exact current content (use the full-file sha256 reported by file.read as expected_base_sha256), prefer a unique anchor over byte ranges, and keep each patch minimal. Every patch call requires an explicit user approval; if a patch is denied, do not retry it unchanged — report the blockage or continue with read-only work. When you report findings from files, cite each one as path:line so the parent can reference the exact location. Do not ask the user questions; if blocked, say what is blocked."
}

/// Build the isolated child prompt: only the delegated description/prompt, never the
/// parent transcript (context isolation is the point).
pub fn build_subagent_task_prompt(description: &str, prompt: &str) -> String {
    let mut out = String::from("Delegated task: ");
    out.push_str(description.trim());
    let detail = prompt.trim();
    if !detail.is_empty() {
        out.push_str("\n\nDetails:\n");
        out.push_str(detail);
    }
    out
}

/// The balanced completed-round prefix of the parent transcript that seeds a
/// delegated child run. The prefix ends at the last complete assistant round
/// (an assistant answer without pending tool calls, or a fully answered
/// tool-call round); an in-flight round with unpaired tool calls is excluded.
/// When the prefix exceeds `max_messages` the most recent messages are kept
/// and the window start advances until the retained slice is balanced again.
/// An empty parent history yields an empty prefix (the child sees only the
/// delegated prompt, as before).
pub fn subagent_context_fork_prefix(messages: &[Message], max_messages: usize) -> Vec<Message> {
    let mut end = messages.len();
    loop {
        if end == 0 {
            return Vec::new();
        }
        if crate::context_engine::is_balanced_cut(&messages[..end])
            && round_completed_at(messages, end)
        {
            break;
        }
        end -= 1;
    }
    let mut start = end.saturating_sub(max_messages);
    while start < end && !crate::context_engine::is_balanced_cut(&messages[start..end]) {
        start += 1;
    }
    messages[start..end].to_vec()
}

/// True when the prefix `messages[..end]` ends on a completed assistant round:
/// a tool observation whose round is balanced, or an assistant message with no
/// proposed tool calls.
fn round_completed_at(messages: &[Message], end: usize) -> bool {
    match &messages[end - 1] {
        message if matches!(message.role, MessageRole::Tool) => true,
        message if matches!(message.role, MessageRole::Assistant) => {
            crate::context_engine::proposed_tool_call_ids(message).is_empty()
        }
        _ => false,
    }
}

/// True when a tool name is permitted for a subagent.
pub fn subagent_tool_allowed(tool_name: &str) -> bool {
    SUBAGENT_ALLOWED_TOOLS.contains(&tool_name)
}

/// True when a tool name is one of the two patch tools a write subagent may
/// call. Anything outside `SUBAGENT_PATCH_TOOLS` (shell, process, computer,
/// browser, file.write, ...) is never part of the write-subagent surface.
pub fn subagent_patch_tool_allowed(tool_name: &str) -> bool {
    SUBAGENT_PATCH_TOOLS.contains(&tool_name)
}

/// Schema of the durable record a delegated child run leaves behind.
pub const SUBAGENT_RUN_SCHEMA: &str = "cindx.agent.subagent-run.v1";

/// Description bound for a durable record. One owner for the bound: the
/// permission path already truncates a write subagent's description to this
/// length, so a record and its approval row always describe the same child.
pub const SUBAGENT_RECORD_DESCRIPTION_MAX_CHARS: usize = 80;

/// How one delegated child finished.
///
/// A child that stopped because the user steered the parent run used to return
/// the same "stage budget exhausted" answer a real Worker-budget exhaustion
/// produces, so a redirected run looked like a child that ran out of budget. The
/// vocabulary separates the two, and separates the step limit, the per-child
/// tool-call cap, cancellation, provider failure, and a panicking child, so the
/// durable record states what actually ended the delegation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentStopReason {
    /// The child answered without a further tool call.
    Completed,
    /// [`SUBAGENT_MAX_STEPS`] model turns elapsed without a final answer.
    StepLimit,
    /// [`SUBAGENT_MAX_TOOL_CALLS`] child tool calls were executed.
    ToolCallBudget,
    /// The parent run's Worker stage budget refused another physical attempt.
    StageBudget,
    /// The user cancelled the parent run.
    Cancelled,
    /// The user steered the parent run, superseding this delegation.
    Steered,
    /// The provider errored or was unavailable.
    ProviderUnavailable,
    /// The child thread panicked.
    Crashed,
    /// The delegation never started: `allow_patches` was refused fail-closed.
    Refused,
}

impl SubagentStopReason {
    /// The wire label recorded on the durable event.
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::StepLimit => "step_limit",
            Self::ToolCallBudget => "tool_call_budget",
            Self::StageBudget => "stage_budget",
            Self::Cancelled => "cancelled",
            Self::Steered => "steered",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::Crashed => "crashed",
            Self::Refused => "refused",
        }
    }
}

/// One delegated child's own report: what it was asked, what it answered, and how
/// it stopped. The stop reason travels with the answer so a child that was
/// steered, cancelled, crashed, or ran out of steps is never reported to the
/// parent — or to the durable record — as something it was not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentChildOutcome {
    pub description: String,
    pub answer: String,
    pub stop_reason: SubagentStopReason,
    /// Model turns actually attempted; a turn the cancel check skipped did no
    /// work and is not counted.
    pub steps: usize,
    /// Child tool calls executed, at most [`SUBAGENT_MAX_TOOL_CALLS`].
    pub tool_calls: usize,
}

impl SubagentChildOutcome {
    pub fn new(
        description: &str,
        answer: impl Into<String>,
        stop_reason: SubagentStopReason,
        steps: usize,
        tool_calls: usize,
    ) -> Self {
        Self {
            description: description.to_string(),
            answer: answer.into(),
            stop_reason,
            steps,
            tool_calls,
        }
    }
}

/// The durable record of one delegated child run.
///
/// The record is bounded: it carries the answer's size and digest but never its
/// text, because the text rides on the persisted `subagent_result` message the
/// parent appends. Together the two make a delegation's outcome survive an app
/// restart — previously the child's history and counters lived only on its
/// thread, and recovery replaced the answer with a synthetic "interrupted" tool
/// observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRunRecord {
    pub schema: String,
    /// The parent's `task` tool-call id this delegation answered.
    pub call_id: String,
    pub description: String,
    pub write: bool,
    pub stop_reason: SubagentStopReason,
    pub steps: usize,
    pub tool_calls: usize,
    pub answer_bytes: usize,
    pub answer_sha256: String,
    /// The model that actually served this child: its own choice when the run
    /// could honor it, otherwise the run's model.
    pub model: String,
    /// The model the delegation asked for, empty when it asked for none. Kept
    /// beside `model` so a fallback is visible instead of silent.
    pub requested_model: String,
    /// The effort tier propagated to the child's model calls.
    pub effort: String,
}

impl SubagentRunRecord {
    /// Builds one record from the child's own outcome plus the delegation
    /// identity only the parent knows, bounding the description and digesting the
    /// answer.
    pub fn new(
        call_id: impl Into<String>,
        outcome: &SubagentChildOutcome,
        write: bool,
        model: &str,
        requested_model: &str,
        effort: &str,
    ) -> Self {
        let bounded_description: String = outcome
            .description
            .trim()
            .chars()
            .take(SUBAGENT_RECORD_DESCRIPTION_MAX_CHARS)
            .collect();
        Self {
            schema: SUBAGENT_RUN_SCHEMA.to_string(),
            call_id: call_id.into(),
            description: if bounded_description.is_empty() {
                "delegated task".to_string()
            } else {
                bounded_description
            },
            write,
            stop_reason: outcome.stop_reason,
            steps: outcome.steps,
            tool_calls: outcome.tool_calls,
            answer_bytes: outcome.answer.len(),
            answer_sha256: subagent_answer_sha256(&outcome.answer),
            model: model.trim().to_string(),
            requested_model: requested_model.trim().to_string(),
            effort: effort.trim().to_string(),
        }
    }

    /// A record is only writable when it stays inside the bounds the child loop
    /// itself enforces, so a counter that outran its cap fails closed instead of
    /// recording an impossible run.
    pub fn contract_is_valid(&self) -> bool {
        self.schema == SUBAGENT_RUN_SCHEMA
            && !self.call_id.trim().is_empty()
            && !self.description.trim().is_empty()
            && self.description.chars().count() <= SUBAGENT_RECORD_DESCRIPTION_MAX_CHARS
            && self.steps <= SUBAGENT_MAX_STEPS
            && self.tool_calls <= SUBAGENT_MAX_TOOL_CALLS
            && self.answer_sha256.len() == 64
            && self
                .answer_sha256
                .chars()
                .all(|digit| digit.is_ascii_hexdigit())
    }

    /// Writes the record onto a durable event's metadata. Returns false, writing
    /// nothing, when the record is not valid.
    pub fn insert_metadata(&self, metadata: &mut Metadata) -> bool {
        if !self.contract_is_valid() {
            return false;
        }
        let steps = self.steps.to_string();
        let tool_calls = self.tool_calls.to_string();
        let answer_bytes = self.answer_bytes.to_string();
        for (key, value) in [
            ("subagent_run_schema", self.schema.as_str()),
            ("subagent_call_id", self.call_id.as_str()),
            ("subagent_description", self.description.as_str()),
            ("subagent_write", if self.write { "true" } else { "false" }),
            ("subagent_stop_reason", self.stop_reason.label()),
            ("subagent_steps", steps.as_str()),
            ("subagent_tool_calls", tool_calls.as_str()),
            ("subagent_answer_bytes", answer_bytes.as_str()),
            ("subagent_answer_sha256", self.answer_sha256.as_str()),
            ("subagent_model", self.model.as_str()),
            ("subagent_effort", self.effort.as_str()),
        ] {
            metadata.insert(key.to_string(), value.to_string());
        }
        // Written only when a delegation actually asked for one, so the common
        // case carries no empty key and a present key means a real request.
        if !self.requested_model.is_empty() {
            metadata.insert(
                "subagent_model_requested".to_string(),
                self.requested_model.clone(),
            );
        }
        true
    }
}

/// The answer digest a record binds, so a persisted `subagent_result` message can
/// be matched to the run record that describes it.
pub fn subagent_answer_sha256(answer: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(answer.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Metadata;

    fn message(role: MessageRole, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            metadata: Metadata::new(),
        }
    }

    fn assistant_tool_call(call_id: &str) -> Message {
        let mut assistant = message(MessageRole::Assistant, "");
        assistant.metadata.insert(
            "raw_tool_calls_json".to_string(),
            format!("[{{\"id\":\"{call_id}\"}}]"),
        );
        assistant
    }

    fn tool_result(call_id: &str) -> Message {
        let mut result = message(MessageRole::Tool, "observation");
        result
            .metadata
            .insert("tool_call_id".to_string(), call_id.to_string());
        result
    }

    #[test]
    fn fork_prefix_contains_the_completed_parent_rounds() {
        let history = vec![
            message(MessageRole::User, "parent request"),
            assistant_tool_call("c1"),
            tool_result("c1"),
            message(MessageRole::Assistant, "parent finding"),
        ];

        let prefix = subagent_context_fork_prefix(&history, SUBAGENT_CONTEXT_FORK_MAX_MESSAGES);

        assert_eq!(prefix, history, "a fully completed transcript seeds whole");
    }

    #[test]
    fn fork_prefix_excludes_an_in_flight_round() {
        let mut history = vec![
            message(MessageRole::User, "parent request"),
            assistant_tool_call("c1"),
            tool_result("c1"),
            message(MessageRole::Assistant, "parent finding"),
        ];
        // The parent is mid-batch: the new turn's tool call has no result yet.
        history.push(message(MessageRole::User, "follow-up"));
        history.push(assistant_tool_call("c2"));

        let prefix = subagent_context_fork_prefix(&history, SUBAGENT_CONTEXT_FORK_MAX_MESSAGES);

        assert_eq!(prefix, &history[..4], "the in-flight round is excluded");
        assert!(crate::context_engine::is_balanced_cut(&prefix));
    }

    #[test]
    fn fork_prefix_caps_message_count_and_stays_balanced() {
        // Fourteen completed rounds of four messages each (56 > the 48 cap).
        let mut history = Vec::new();
        for round in 0..14 {
            history.push(message(MessageRole::User, &format!("request {round}")));
            history.push(assistant_tool_call(&format!("c{round}")));
            history.push(tool_result(&format!("c{round}")));
            history.push(message(MessageRole::Assistant, &format!("answer {round}")));
        }
        assert_eq!(history.len(), 56);

        let prefix = subagent_context_fork_prefix(&history, SUBAGENT_CONTEXT_FORK_MAX_MESSAGES);

        assert_eq!(prefix.len(), SUBAGENT_CONTEXT_FORK_MAX_MESSAGES);
        assert!(crate::context_engine::is_balanced_cut(&prefix));
        assert_eq!(
            prefix.last(),
            history.last(),
            "the cap keeps the most recent messages"
        );
    }

    #[test]
    fn fork_prefix_degenerates_to_empty_without_parent_history() {
        assert!(subagent_context_fork_prefix(&[], SUBAGENT_CONTEXT_FORK_MAX_MESSAGES).is_empty());
        // Only an in-flight round: nothing completed, nothing to seed.
        let in_flight = vec![
            message(MessageRole::User, "request"),
            assistant_tool_call("c1"),
        ];
        assert!(
            subagent_context_fork_prefix(&in_flight, SUBAGENT_CONTEXT_FORK_MAX_MESSAGES).is_empty()
        );
    }

    #[test]
    fn subagent_prompt_is_isolated_and_bounded() {
        let prompt = build_subagent_task_prompt("find the parser", "look in crates/");
        assert!(prompt.starts_with("Delegated task: find the parser"));
        assert!(prompt.contains("look in crates/"));
        assert!(subagent_system_prompt().contains("isolated"));
    }

    #[test]
    fn subagent_tool_policy_is_read_only() {
        assert!(subagent_tool_allowed("file.read"));
        assert!(subagent_tool_allowed("file.glob"));
        assert!(subagent_tool_allowed("web.search"));
        assert!(subagent_tool_allowed("web.fetch"));
        assert!(!subagent_tool_allowed("file.write"));
        assert!(!subagent_tool_allowed("shell.run"));
        assert!(!subagent_tool_allowed("task"));
        assert!(!subagent_tool_allowed("todo.write"));
    }

    #[test]
    fn subagent_patch_policy_is_exactly_the_patch_pair() {
        assert!(subagent_patch_tool_allowed("file.patch"));
        assert!(subagent_patch_tool_allowed("file.patch_batch"));
        for denied in [
            "file.write",
            "file.delete",
            "shell.run",
            "process.start",
            "computer.click",
            "browser.open",
            "task",
            "todo.write",
            "image.generate",
        ] {
            assert!(
                !subagent_patch_tool_allowed(denied),
                "{denied} must never enter the write-subagent surface"
            );
        }
        // The patch pair stays disjoint from the read-only whitelist, and the
        // read-only policy does not silently admit them.
        for patch in SUBAGENT_PATCH_TOOLS {
            assert!(!subagent_tool_allowed(patch));
        }
    }

    #[test]
    fn subagent_write_prompt_keeps_isolation_and_adds_patch_discipline() {
        let prompt = subagent_write_system_prompt();
        assert!(prompt.contains("isolated"));
        assert!(prompt.contains("file.patch"));
        assert!(prompt.contains("expected_base_sha256"));
        assert!(prompt.contains("explicit user approval"));
        assert!(prompt.contains("denied"));
    }

    /// The durable record carries a delegation's bounded identity and the digest
    /// of its answer — never the answer text, which rides on the persisted
    /// `subagent_result` message instead.
    #[test]
    fn a_subagent_run_record_writes_its_bounded_facts_to_event_metadata() {
        let answer = "The retry path is bounded.";
        let record = SubagentRunRecord::new(
            "call_task_1",
            &SubagentChildOutcome::new(
                "Audit the retry path",
                answer,
                SubagentStopReason::Completed,
                3,
                5,
            ),
            true,
            "qwen3.8-max",
            "gpt-5-mini",
            "high",
        );
        assert!(record.contract_is_valid());

        let mut metadata = Metadata::new();
        assert!(record.insert_metadata(&mut metadata));
        assert_eq!(
            metadata.get("subagent_run_schema").map(String::as_str),
            Some(SUBAGENT_RUN_SCHEMA)
        );
        assert_eq!(
            metadata.get("subagent_call_id").map(String::as_str),
            Some("call_task_1")
        );
        assert_eq!(
            metadata.get("subagent_stop_reason").map(String::as_str),
            Some("completed")
        );
        assert_eq!(
            metadata.get("subagent_write").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            metadata.get("subagent_steps").map(String::as_str),
            Some("3")
        );
        assert_eq!(
            metadata.get("subagent_tool_calls").map(String::as_str),
            Some("5")
        );
        assert_eq!(
            metadata.get("subagent_answer_bytes").map(String::as_str),
            Some("26")
        );
        assert_eq!(
            metadata.get("subagent_answer_sha256").map(String::as_str),
            Some(subagent_answer_sha256(answer).as_str())
        );
        assert_eq!(
            metadata.get("subagent_model").map(String::as_str),
            Some("qwen3.8-max"),
            "the record names the model that actually served the child"
        );
        assert_eq!(
            metadata.get("subagent_model_requested").map(String::as_str),
            Some("gpt-5-mini"),
            "an unhonored request stays visible next to the model that ran"
        );
        assert_eq!(
            metadata.get("subagent_effort").map(String::as_str),
            Some("high")
        );
        assert!(
            !metadata
                .values()
                .any(|value| value.contains("retry path is")),
            "the answer text must not ride on the record"
        );
    }

    #[test]
    fn a_record_bounds_its_description_and_never_records_an_empty_one() {
        let long = "x".repeat(SUBAGENT_RECORD_DESCRIPTION_MAX_CHARS * 3);
        let bounded = SubagentRunRecord::new(
            "call_1",
            &SubagentChildOutcome::new(
                &long,
                "",
                SubagentStopReason::StepLimit,
                SUBAGENT_MAX_STEPS,
                0,
            ),
            false,
            "m",
            "",
            "fast",
        );
        assert_eq!(
            bounded.description.chars().count(),
            SUBAGENT_RECORD_DESCRIPTION_MAX_CHARS
        );
        assert!(bounded.contract_is_valid());
        // A delegation that asked for no model carries no request key, so a
        // present key always means a real request.
        let mut bounded_metadata = Metadata::new();
        assert!(bounded.insert_metadata(&mut bounded_metadata));
        assert_eq!(
            bounded_metadata.get("subagent_model").map(String::as_str),
            Some("m")
        );
        assert!(!bounded_metadata.contains_key("subagent_model_requested"));

        let blank = SubagentRunRecord::new(
            "call_1",
            &SubagentChildOutcome::new("   ", "", SubagentStopReason::Refused, 0, 0),
            false,
            "m",
            "",
            "fast",
        );
        assert_eq!(
            blank.description, "delegated task",
            "a blank description falls back to the delegation default rather than recording nothing"
        );
        assert!(blank.contract_is_valid());
        assert_eq!(
            blank.stop_reason.label(),
            "refused",
            "a delegation that never started is recorded as refused, not completed"
        );
    }

    /// Counters that outran the caps the child loop itself enforces mean the
    /// record was built by a bug, so it fails closed and writes nothing.
    #[test]
    fn a_record_with_counters_past_the_child_caps_fails_closed() {
        let mut record = SubagentRunRecord::new(
            "call_1",
            &SubagentChildOutcome::new(
                "bounded work",
                "answer",
                SubagentStopReason::ToolCallBudget,
                1,
                SUBAGENT_MAX_TOOL_CALLS,
            ),
            false,
            "m",
            "",
            "fast",
        );
        assert!(record.contract_is_valid());

        record.tool_calls = SUBAGENT_MAX_TOOL_CALLS + 1;
        assert!(!record.contract_is_valid());
        let mut metadata = Metadata::new();
        assert!(!record.insert_metadata(&mut metadata));
        assert!(metadata.is_empty(), "an invalid record writes nothing");

        record.tool_calls = SUBAGENT_MAX_TOOL_CALLS;
        record.steps = SUBAGENT_MAX_STEPS + 1;
        assert!(!record.contract_is_valid());

        record.steps = 1;
        record.call_id = " ".to_string();
        assert!(
            !record.contract_is_valid(),
            "a record must bind the delegation it describes"
        );
    }

    /// Steer and stage-budget exhaustion are the pair that used to be confounded
    /// in the child's answer, so their labels must stay distinct and stable.
    #[test]
    fn every_stop_reason_has_a_distinct_stable_wire_label() {
        let reasons = [
            SubagentStopReason::Completed,
            SubagentStopReason::StepLimit,
            SubagentStopReason::ToolCallBudget,
            SubagentStopReason::StageBudget,
            SubagentStopReason::Cancelled,
            SubagentStopReason::Steered,
            SubagentStopReason::ProviderUnavailable,
            SubagentStopReason::Crashed,
            SubagentStopReason::Refused,
        ];
        let labels = reasons
            .iter()
            .map(|reason| reason.label())
            .collect::<Vec<_>>();
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "labels: {labels:?}");
        assert_ne!(
            SubagentStopReason::Steered.label(),
            SubagentStopReason::StageBudget.label()
        );
        assert!(
            labels
                .iter()
                .all(|label| !label.is_empty() && !label.contains(' ')),
            "labels are wire values: {labels:?}"
        );
    }
}
