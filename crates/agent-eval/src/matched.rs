//! Matched, position-balanced multi-arm comparison driver (Phase 4 harness).
//!
//! The frozen protocol requires position-balanced matched arms with every
//! failure retained in the denominator. This driver runs every case across every
//! arm, rotating the arm order per case so no arm always runs first (cancelling
//! order and warm-cache effects), gives each `(case, arm)` an isolated workspace
//! and a fresh provider, and retains every run — pass, fail, or error.
//!
//! Arms are caller-defined (typically a model or configuration label). Wiring
//! arms that vary the *effort tier* or *subagent delegation* additionally needs
//! the product-path fidelity work tracked in EVALUATION.md; this driver supplies
//! the matched/position-balanced scaffolding those arms plug into.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::runner::{run_case, CaseReport, EvalModelProvider};
use crate::EvalCase;

/// One arm's run for one case, with the position it executed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmRun {
    pub arm: String,
    /// Execution position within the case (0 = first). Balanced across cases.
    pub position: usize,
    pub report: CaseReport,
}

/// Every arm's run for one case, in execution order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedCaseReport {
    pub case_id: String,
    pub arms: Vec<ArmRun>,
}

/// A complete matched, position-balanced comparison over a suite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedArmReport {
    pub arms: Vec<String>,
    pub cases: Vec<MatchedCaseReport>,
}

impl MatchedArmReport {
    /// Per-arm pass rate over the full denominator (every case is counted for
    /// every arm, including errors and failures — nothing is dropped).
    pub fn pass_rate(&self, arm: &str) -> Option<(usize, usize)> {
        let mut passed = 0usize;
        let mut total = 0usize;
        for case in &self.cases {
            for run in &case.arms {
                if run.arm == arm {
                    total += 1;
                    if run.report.passed {
                        passed += 1;
                    }
                }
            }
        }
        (total > 0).then_some((passed, total))
    }

    /// True when each arm occupied each execution position the same number of
    /// times (within one), i.e. the order is position-balanced.
    pub fn is_position_balanced(&self) -> bool {
        use std::collections::BTreeMap;
        let mut counts: BTreeMap<(String, usize), usize> = BTreeMap::new();
        for case in &self.cases {
            for run in &case.arms {
                *counts.entry((run.arm.clone(), run.position)).or_insert(0) += 1;
            }
        }
        let arm_count = self.arms.len().max(1);
        for arm in &self.arms {
            let per_position: Vec<usize> = (0..arm_count)
                .map(|position| counts.get(&(arm.clone(), position)).copied().unwrap_or(0))
                .collect();
            let min = per_position.iter().copied().min().unwrap_or(0);
            let max = per_position.iter().copied().max().unwrap_or(0);
            if max.saturating_sub(min) > 1 {
                return false;
            }
        }
        true
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("matched arm report serializes")
    }
}

/// Run every case across every arm with position balancing. For case index `i`
/// the arm order is rotated by `i`, so each arm runs in each position across the
/// suite. `make_provider` builds a fresh provider for the named arm; each
/// `(case, arm)` runs in an isolated workspace so runs never share state.
pub fn run_matched_arms<P, F>(
    cases: &[EvalCase],
    arms: &[String],
    workspace_root: &Path,
    mut make_provider: F,
) -> MatchedArmReport
where
    P: EvalModelProvider,
    F: FnMut(&str) -> P,
{
    let arm_count = arms.len();
    let mut case_reports = Vec::new();
    for (case_index, case) in cases.iter().enumerate() {
        let mut arm_runs = Vec::new();
        for position in 0..arm_count {
            let arm_index = (case_index + position) % arm_count.max(1);
            let arm = &arms[arm_index];
            let arm_root = workspace_root.join(format!("arm-{arm_index}"));
            let mut provider = make_provider(arm);
            let report = run_case(case, &mut provider, &arm_root);
            arm_runs.push(ArmRun {
                arm: arm.clone(),
                position,
                report,
            });
        }
        case_reports.push(MatchedCaseReport {
            case_id: case.id.clone(),
            arms: arm_runs,
        });
    }
    MatchedArmReport {
        arms: arms.to_vec(),
        cases: case_reports,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{ScriptedProvider, ScriptedStep};
    use crate::{CaseBudget, CaseCategory};

    fn case(id: &str) -> EvalCase {
        EvalCase {
            id: id.to_string(),
            category: CaseCategory::Output,
            prompt: "answer".to_string(),
            fixture: vec![],
            allowed_tools: vec![],
            postconditions: vec![],
            budget: CaseBudget {
                max_turns: 2,
                max_tool_calls: 0,
            },
        }
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cindx-eval-matched-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn matched_arms_are_position_balanced_and_retain_every_run() {
        let root = temp_root("balance");
        let cases = vec![case("c1"), case("c2"), case("c3"), case("c4")];
        let arms = vec!["a".to_string(), "b".to_string()];
        let report = run_matched_arms(&cases, &arms, &root, |_arm| {
            ScriptedProvider::new(vec![ScriptedStep::Final("done".to_string())])
        });

        // Every case ran every arm; nothing was dropped from the denominator.
        assert_eq!(report.cases.len(), 4);
        for case in &report.cases {
            assert_eq!(case.arms.len(), 2);
        }
        assert_eq!(report.pass_rate("a"), Some((4, 4)));
        assert_eq!(report.pass_rate("b"), Some((4, 4)));
        // Position balancing: each arm ran first and second an equal number of
        // times across the four cases.
        assert!(report.is_position_balanced());
        // Case 0 starts with arm a; case 1 starts with arm b (rotation).
        assert_eq!(report.cases[0].arms[0].arm, "a");
        assert_eq!(report.cases[1].arms[0].arm, "b");
    }
}
