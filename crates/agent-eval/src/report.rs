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
        let total_model_calls: usize = self
            .cases
            .iter()
            .map(|case| case.receipts.model_calls)
            .sum();
        let total_tokens: u64 = self
            .cases
            .iter()
            .map(|case| case.receipts.total_tokens)
            .sum();
        let mut out = format!(
            "suite: {}/{} cases passed | model_calls={total_model_calls} tokens={total_tokens}\n",
            self.passed_count, self.total
        );
        for case in &self.cases {
            let status = if case.passed { "PASS" } else { "FAIL" };
            let checks_passed = case.checks.iter().filter(|check| check.passed).count();
            let mut line = format!(
                "- [{status}] {} checks={}/{} turns={} tool_calls={} model_calls={} tokens={}",
                case.id,
                checks_passed,
                case.checks.len(),
                case.turns,
                case.tool_calls,
                case.receipts.model_calls,
                case.receipts.total_tokens
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
