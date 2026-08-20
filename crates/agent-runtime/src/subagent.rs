//! Borrowed from opencode `tool/task.ts`, pi `subagent`, and deepseek-harness
//! `subagent`/`goal-round-driver`: a subagent is an isolated child run that sees
//! only the delegated task, uses a restricted read-only tool set, and returns its
//! final text to the parent. This module holds the deterministic policy; the child
//! execution is wired by the desktop loop.

pub const SUBAGENT_MAX_STEPS: usize = 8;

/// Tools a subagent may use: read-only discovery. Effectful, delegation, and
/// working-memory tools are denied (mirrors opencode deriveSubagentSessionPermission).
pub const SUBAGENT_ALLOWED_TOOLS: &[&str] = &[
    "file.read",
    "file.reads",
    "file.list",
    "file.search",
    "web.search",
];

pub fn subagent_system_prompt() -> &'static str {
    "You are a subagent handling one delegated task in an isolated context. Work it to completion using only read-only discovery tools, then answer concisely with the concrete result (findings, file paths, or a short summary). Do not ask the user questions; if blocked, say what is blocked."
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

/// True when a tool name is permitted for a subagent.
pub fn subagent_tool_allowed(tool_name: &str) -> bool {
    SUBAGENT_ALLOWED_TOOLS.contains(&tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(subagent_tool_allowed("web.search"));
        assert!(!subagent_tool_allowed("file.write"));
        assert!(!subagent_tool_allowed("shell.run"));
        assert!(!subagent_tool_allowed("task"));
        assert!(!subagent_tool_allowed("todo.write"));
    }
}
