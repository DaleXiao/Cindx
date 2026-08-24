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
