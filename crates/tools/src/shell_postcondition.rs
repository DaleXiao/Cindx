use std::fs;
use std::path::Path;

use agent_core::{
    PostconditionVerifierKind, ToolInvocation, ToolOutcomeStatus, ToolPostconditionEvidence,
    ToolResult, ToolSpec,
};

use crate::tool_contract_v2::shell_run_spec;
use crate::{parse_input, resolve_workspace_path, resolve_workspace_read_path};

pub(crate) fn tool_spec(default_timeout: u64, max_timeout: u64) -> ToolSpec {
    shell_run_spec(default_timeout, max_timeout)
        .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceQualityCheckV1)
}

pub(crate) fn workspace_scope(workspace_root: &Path, cwd: &str) -> String {
    resolved_workspace_cwd(workspace_root, cwd)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| cwd.to_string())
}

pub(crate) fn quality_check(
    workspace_root: &Path,
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> Option<ToolPostconditionEvidence> {
    if result.invocation_id != invocation.id
        || !matches!(result.status, ToolOutcomeStatus::Succeeded)
    {
        return None;
    }
    let input = parse_input(&invocation.input_json);
    let command = input.get("command")?.trim();
    let cwd = input.get("cwd").map(String::as_str).unwrap_or(".");
    if !quality_check_command(command) || !cwd_is_workspace_root(workspace_root, cwd) {
        return None;
    }
    let structured =
        serde_json::from_str::<serde_json::Value>(result.structured_output_json.as_deref()?)
            .ok()?;
    if structured.get("schema")?.as_str()? != "cindx.shell-result.v1"
        || structured.get("cwd")?.as_str()? != cwd
        || structured.get("exit_code")?.as_i64()? != 0
        || structured.get("termination")?.as_str()? != "exit"
    {
        return None;
    }
    Some(ToolPostconditionEvidence {
        kind: PostconditionVerifierKind::WorkspaceQualityCheckV1,
        target_input_json: serde_json::json!({ "path": "." }).to_string(),
    })
}

fn cwd_is_workspace_root(workspace_root: &Path, cwd: &str) -> bool {
    let Ok(root) = fs::canonicalize(workspace_root) else {
        return false;
    };
    resolved_workspace_cwd(workspace_root, cwd).is_some_and(|resolved| resolved == root)
}

fn resolved_workspace_cwd(workspace_root: &Path, cwd: &str) -> Option<std::path::PathBuf> {
    resolve_workspace_path(workspace_root, cwd)
        .and_then(|path| resolve_workspace_read_path(workspace_root, &path))
        .ok()
}

fn quality_check_command(command: &str) -> bool {
    let Some(words) = direct_shell_words(command) else {
        return false;
    };
    let Some((executable, arguments)) = words.split_first() else {
        return false;
    };
    let executable = executable.to_ascii_lowercase();
    let arguments = arguments
        .iter()
        .map(|argument| argument.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if arguments.iter().any(|argument| {
        matches!(
            argument.as_str(),
            "--help"
                | "-h"
                | "--version"
                | "--list"
                | "--list-tests"
                | "--listtests"
                | "--collect-only"
                | "--collectonly"
        )
    }) {
        return false;
    }
    match executable.as_str() {
        "cargo" => match arguments.first().map(String::as_str) {
            Some("test" | "check" | "build" | "clippy") => true,
            Some("fmt") => arguments.iter().any(|argument| argument == "--check"),
            _ => false,
        },
        "swift" => matches!(
            arguments.first().map(String::as_str),
            Some("test" | "build")
        ),
        "go" => matches!(arguments.first().map(String::as_str), Some("test")),
        "pytest" | "pytest-3" | "vitest" | "jest" => true,
        "npm" | "pnpm" | "yarn" | "bun" => package_quality_command(&arguments),
        "make" => arguments
            .first()
            .is_some_and(|target| quality_action(target)),
        _ => false,
    }
}

fn package_quality_command(arguments: &[String]) -> bool {
    match arguments {
        [action, ..] if quality_action(action) => true,
        [run, action, ..] if run == "run" && quality_action(action) => true,
        _ => false,
    }
}

fn quality_action(value: &str) -> bool {
    ["test", "check", "build", "lint", "typecheck", "type-check"]
        .iter()
        .any(|action| value == *action || value.starts_with(&format!("{action}:")))
}

fn direct_shell_words(command: &str) -> Option<Vec<String>> {
    if command.trim().is_empty()
        || command.len() > 2_000
        || command.contains("$(")
        || command.contains('`')
        || command.contains('#')
    {
        return None;
    }
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in command.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else {
                if matches!(active_quote, '"') && character == '$' {
                    return None;
                }
                word.push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character.is_whitespace() {
            if matches!(character, '\n' | '\r') {
                return None;
            }
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else if matches!(
            character,
            ';' | '|' | '&' | '(' | ')' | '{' | '}' | '<' | '>' | '$' | '*' | '?' | '~'
        ) {
            return None;
        } else {
            word.push(character);
        }
    }
    if escaped || quote.is_some() {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    (!words.is_empty()).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Metadata, TaskId, ToolCallId};

    fn invocation(command: &str) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId("shell-postcondition-test".to_string()),
            task_id: TaskId("task".to_string()),
            tool_name: "shell.run".to_string(),
            input_json: serde_json::json!({"command": command}).to_string(),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        }
    }

    fn completed_result(
        invocation: &ToolInvocation,
        cwd: &str,
        status: ToolOutcomeStatus,
        exit_code: i64,
        termination: &str,
    ) -> ToolResult {
        let mut result = ToolResult::text(
            invocation.id.clone(),
            status,
            "check completed",
            Metadata::new(),
        );
        result.structured_output_json = Some(
            serde_json::json!({
                "schema": "cindx.shell-result.v1",
                "cwd": cwd,
                "exit_code": exit_code,
                "termination": termination,
                "stdout_bytes": 0,
                "stderr_bytes": 0,
                "output_truncated": false,
                "stdout_artifact": null,
                "stderr_artifact": null,
                "stdout_complete": true,
                "stderr_complete": true
            })
            .to_string(),
        );
        result
    }

    #[test]
    fn direct_successful_quality_commands_produce_typed_evidence() {
        let workspace = tempfile::tempdir().expect("workspace should be created");
        for command in [
            "cargo test",
            "cargo check --workspace",
            "cargo build --release",
            "cargo clippy --all-targets",
            "cargo fmt --check",
            "swift test",
            "go test ./...",
            "pytest -q",
            "vitest run",
            "jest --runInBand",
            "npm test",
            "npm run build",
            "pnpm lint",
            "yarn check",
            "bun test",
            "make test",
        ] {
            let request = invocation(command);
            let result = completed_result(&request, ".", ToolOutcomeStatus::Succeeded, 0, "exit");
            let evidence = quality_check(workspace.path(), &request, &result)
                .unwrap_or_else(|| panic!("{command} should produce quality evidence"));
            assert_eq!(
                evidence.kind,
                PostconditionVerifierKind::WorkspaceQualityCheckV1
            );
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&evidence.target_input_json)
                    .expect("quality evidence target should be JSON"),
                serde_json::json!({ "path": "." })
            );
        }
    }

    #[test]
    fn shell_text_and_indirection_cannot_claim_quality_evidence() {
        let workspace = tempfile::tempdir().expect("workspace should be created");
        for command in [
            "echo test",
            "printf 'cargo test'",
            "echo ok # cargo test",
            "eval 'cargo test'",
            "source ./check.sh",
            "sh -c 'cargo test'",
            "cargo test $(echo unit)",
            "cargo test `echo unit`",
            "cargo test && echo done",
            "cargo test; echo done",
            "cargo test | tee output.log",
            "cargo test\necho done",
            "CI=1 cargo test",
            "cargo test --help",
            "pytest --collect-only",
        ] {
            let request = invocation(command);
            let result = completed_result(&request, ".", ToolOutcomeStatus::Succeeded, 0, "exit");
            assert!(
                quality_check(workspace.path(), &request, &result).is_none(),
                "{command} must not produce quality evidence"
            );
        }
    }

    #[test]
    fn quality_evidence_requires_root_cwd_and_true_success_contract() {
        let workspace = tempfile::tempdir().expect("workspace should be created");
        fs::create_dir_all(workspace.path().join("nested")).expect("nested workspace should exist");
        let mut nested = invocation("cargo test");
        nested.input_json =
            serde_json::json!({ "command": "cargo test", "cwd": "nested" }).to_string();
        let nested_result =
            completed_result(&nested, "nested", ToolOutcomeStatus::Succeeded, 0, "exit");
        assert!(quality_check(workspace.path(), &nested, &nested_result).is_none());

        let request = invocation("cargo test");
        for (status, exit_code, termination) in [
            (ToolOutcomeStatus::Failed, 1, "exit"),
            (ToolOutcomeStatus::Succeeded, 1, "exit"),
            (ToolOutcomeStatus::Succeeded, 0, "timeout"),
            (ToolOutcomeStatus::Succeeded, 0, "cancelled"),
            (ToolOutcomeStatus::Succeeded, 0, "signal"),
        ] {
            let result = completed_result(&request, ".", status, exit_code, termination);
            assert!(quality_check(workspace.path(), &request, &result).is_none());
        }

        let mut malformed =
            completed_result(&request, ".", ToolOutcomeStatus::Succeeded, 0, "exit");
        malformed.structured_output_json = Some(r#"{"schema":"other"}"#.to_string());
        assert!(quality_check(workspace.path(), &request, &malformed).is_none());
    }
}
