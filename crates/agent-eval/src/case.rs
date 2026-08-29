//! Evaluation case schema. Pure data: serde types plus a suite parser.

use serde::{Deserialize, Serialize};

/// The behavioral family a case exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseCategory {
    CodeEdit,
    MultiStep,
    Recovery,
    Shell,
    Output,
}

impl CaseCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeEdit => "code_edit",
            Self::MultiStep => "multi_step",
            Self::Recovery => "recovery",
            Self::Shell => "shell",
            Self::Output => "output",
        }
    }
}

/// One file materialized into the case workspace before the loop starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureFile {
    pub path: String,
    pub content: String,
}

/// A deterministic check judged after the loop finishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Postcondition {
    FileContains {
        path: String,
        needle: String,
    },
    FileEquals {
        path: String,
        content: String,
    },
    /// Runs `cmd` through `/bin/sh` inside the case workspace and compares
    /// the exit code.
    CommandExitCode {
        cmd: String,
        code: i32,
    },
    OutputContains {
        needle: String,
    },
}

/// Hard per-case budgets applied by the runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseBudget {
    pub max_turns: usize,
    pub max_tool_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalCase {
    pub id: String,
    pub category: CaseCategory,
    pub prompt: String,
    #[serde(default)]
    pub fixture: Vec<FixtureFile>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub postconditions: Vec<Postcondition>,
    pub budget: CaseBudget,
}

/// Parse a suite file: a JSON array of cases.
pub fn parse_suite(json: &str) -> Result<Vec<EvalCase>, serde_json::Error> {
    serde_json::from_str(json)
}
