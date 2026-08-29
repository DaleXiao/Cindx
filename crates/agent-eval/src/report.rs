//! Suite-level aggregation over per-case reports.

use serde::{Deserialize, Serialize};

use crate::runner::CaseReport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteReport {
    pub cases: Vec<CaseReport>,
    pub passed_count: usize,
    pub total: usize,
}

impl SuiteReport {
    pub fn new(cases: Vec<CaseReport>) -> Self {
        let passed_count = cases.iter().filter(|case| case.passed).count();
        let total = cases.len();
        Self {
            cases,
            passed_count,
            total,
        }
    }

    pub fn all_passed(&self) -> bool {
        self.passed_count == self.total
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("suite report serializes")
    }

    /// One bounded status line per case plus the aggregate.
    pub fn summary(&self) -> String {
        let mut out = format!("suite: {}/{} cases passed\n", self.passed_count, self.total);
        for case in &self.cases {
            let status = if case.passed { "PASS" } else { "FAIL" };
            let checks_passed = case.checks.iter().filter(|check| check.passed).count();
            let mut line = format!(
                "- [{status}] {} checks={}/{} turns={} tool_calls={}",
                case.id,
                checks_passed,
                case.checks.len(),
                case.turns,
                case.tool_calls
            );
            if let Some(error) = &case.error {
                line.push_str(&format!(" error={error}"));
            }
            out.push_str(&line);
            out.push('\n');
        }
        out
    }
}
