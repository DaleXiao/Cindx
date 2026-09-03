//! Borrowed from opencode `tool/task.ts`, pi `subagent`, and deepseek-harness
//! `subagent`/`goal-round-driver`: a subagent is an isolated child run that sees
//! the delegated task plus a bounded seed of the parent's completed rounds, uses
//! a restricted read-only tool set, and returns its final text to the parent.
//! This module holds the deterministic policy; the child execution is wired by
//! the desktop loop.

use agent_core::{Message, MessageRole};

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
}
