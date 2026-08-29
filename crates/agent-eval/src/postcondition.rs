//! Pure postcondition checks over a case workspace and the final answer.
//! Every check is deterministic and fails closed on missing or unreadable
//! evidence.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::case::Postcondition;

/// Maximum characters of command output retained in a check detail.
pub const CHECK_OUTPUT_LIMIT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub postcondition: Postcondition,
    pub passed: bool,
    pub detail: String,
}

/// Judge every postcondition in order against the workspace root and the
/// run's final answer.
pub fn check_postconditions(
    workspace_root: &Path,
    final_answer: &str,
    postconditions: &[Postcondition],
) -> Vec<CheckResult> {
    postconditions
        .iter()
        .map(|postcondition| check_one(workspace_root, final_answer, postcondition))
        .collect()
}

fn check_one(
    workspace_root: &Path,
    final_answer: &str,
    postcondition: &Postcondition,
) -> CheckResult {
    match postcondition {
        Postcondition::FileContains { path, needle } => {
            match std::fs::read_to_string(workspace_root.join(path)) {
                Err(error) => failed(postcondition, format!("cannot read {path}: {error}")),
                Ok(content) if content.contains(needle) => {
                    passed(postcondition, format!("needle present in {path}"))
                }
                Ok(content) => failed(
                    postcondition,
                    format!("needle not present in {path} ({} bytes)", content.len()),
                ),
            }
        }
        Postcondition::FileEquals { path, content } => {
            match std::fs::read_to_string(workspace_root.join(path)) {
                Err(error) => failed(postcondition, format!("cannot read {path}: {error}")),
                Ok(actual) if actual == *content => {
                    passed(postcondition, format!("{path} matches exactly"))
                }
                Ok(actual) => failed(
                    postcondition,
                    format!(
                        "{path} differs: {} bytes, expected {} bytes",
                        actual.len(),
                        content.len()
                    ),
                ),
            }
        }
        Postcondition::CommandExitCode { cmd, code } => match Command::new("/bin/sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(workspace_root)
            .output()
        {
            Err(error) => failed(postcondition, format!("command failed to start: {error}")),
            Ok(output) => {
                let actual = output.status.code().unwrap_or(-1);
                if actual == *code {
                    passed(postcondition, format!("exit code {actual}"))
                } else {
                    let mut observed = String::from_utf8_lossy(&output.stdout).into_owned();
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stderr.trim().is_empty() {
                        if !observed.is_empty() {
                            observed.push('\n');
                        }
                        observed.push_str(stderr.trim_end());
                    }
                    failed(
                        postcondition,
                        format!(
                            "exit code {actual}, expected {code}; output: {}",
                            bound(&observed)
                        ),
                    )
                }
            }
        },
        Postcondition::OutputContains { needle } => {
            if final_answer.contains(needle) {
                passed(postcondition, "needle present in final answer".to_string())
            } else {
                failed(
                    postcondition,
                    format!(
                        "needle not present in final answer ({} chars)",
                        final_answer.chars().count()
                    ),
                )
            }
        }
    }
}

fn passed(postcondition: &Postcondition, detail: String) -> CheckResult {
    CheckResult {
        postcondition: postcondition.clone(),
        passed: true,
        detail,
    }
}

fn failed(postcondition: &Postcondition, detail: String) -> CheckResult {
    CheckResult {
        postcondition: postcondition.clone(),
        passed: false,
        detail,
    }
}

fn bound(value: &str) -> String {
    const MARKER: &str = "...[truncated]";
    let count = value.chars().count();
    if count <= CHECK_OUTPUT_LIMIT {
        return value.to_string();
    }
    let retained = CHECK_OUTPUT_LIMIT.saturating_sub(MARKER.chars().count());
    let mut out: String = value.chars().take(retained).collect();
    out.push_str(MARKER);
    out
}
