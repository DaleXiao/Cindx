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

/// Everything one `(case, arm)` cell needs to run: the case, the arm label,
/// the cell's isolated workspace root, and the position-balancing bookkeeping.
pub struct MatchedCellContext<'a> {
    pub case: &'a EvalCase,
    pub case_index: usize,
    pub arm: &'a str,
    pub arm_index: usize,
    pub position: usize,
    /// Per-cell isolated root: `<workspace_root>/case-<i>/<arm>`.
    pub cell_root: std::path::PathBuf,
}

/// The matched/position-balanced core with a caller-supplied cell executor.
/// Rotation and denominator semantics are identical to [`run_matched_arms`]:
/// for case index `i` the arm order rotates by `i`, and every cell — pass,
/// fail, or error — is retained. Product-path drivers plug in here (Phase 4
/// fidelity); the provider-shaped convenience wrapper stays for kernel runs.
pub fn run_matched_cells<E>(
    cases: &[EvalCase],
    arms: &[String],
    workspace_root: &Path,
    mut run_cell: E,
) -> MatchedArmReport
where
    E: FnMut(MatchedCellContext<'_>) -> CaseReport,
{
    let arm_count = arms.len();
    let mut case_reports = Vec::new();
    for (case_index, case) in cases.iter().enumerate() {
        let mut arm_runs = Vec::new();
        for position in 0..arm_count {
            let arm_index = (case_index + position) % arm_count.max(1);
            let arm = &arms[arm_index];
            let cell_root = workspace_root
                .join(format!("case-{case_index}"))
                .join(arm.as_str());
            let report = run_cell(MatchedCellContext {
                case,
                case_index,
                arm,
                arm_index,
                position,
                cell_root,
            });
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

/// Run every case across every arm with position balancing. For case index `i`
/// the arm order is rotated by `i`, so each arm runs in each position across the
/// suite. `make_provider` builds a fresh provider for the named arm; each
/// arm reuses its historical `arm-<index>` workspace root (the kernel-runner
/// layout, kept for the crate's contract tests; product-path drivers use
/// [`run_matched_cells`], which isolates every `(case, arm)` cell in its own
/// root).
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
    run_matched_cells(cases, arms, workspace_root, |cell| {
        let arm_root = workspace_root.join(format!("arm-{}", cell.arm_index));
        let mut provider = make_provider(cell.arm);
        run_case(cell.case, &mut provider, &arm_root)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{CaseReport, ScriptedProvider, ScriptedStep};
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

    #[test]
    fn matched_cells_isolate_every_case_arm_pair_and_keep_rotation() {
        let root = temp_root("cells");
        let cases = vec![case("c1"), case("c2"), case("c3")];
        let arms = vec!["single-fast".to_string(), "subagent-fast".to_string()];
        let mut seen_roots = Vec::new();
        let report = run_matched_cells(&cases, &arms, &root, |cell| {
            seen_roots.push(cell.cell_root.clone());
            // The cell root is per (case, arm): it carries both identities.
            assert!(cell.cell_root.starts_with(&root));
            assert!(cell.cell_root.ends_with(cell.arm));
            if cell.case.id == "c2" && cell.arm == "single-fast" {
                CaseReport {
                    id: cell.case.id.clone(),
                    passed: false,
                    checks: vec![],
                    tool_calls: 0,
                    turns: 0,
                    error: Some("cell failure retained".to_string()),
                    receipts: Default::default(),
                }
            } else {
                CaseReport {
                    id: cell.case.id.clone(),
                    passed: true,
                    checks: vec![],
                    tool_calls: 0,
                    turns: 0,
                    error: None,
                    receipts: Default::default(),
                }
            }
        });

        // Every (case, arm) cell ran exactly once in its own root.
        assert_eq!(seen_roots.len(), 6);
        let unique: std::collections::BTreeSet<_> = seen_roots.iter().collect();
        assert_eq!(unique.len(), 6, "cell roots must not be shared");
        // A failing cell stays in the denominator.
        assert_eq!(report.pass_rate("single-fast"), Some((2, 3)));
        assert_eq!(report.pass_rate("subagent-fast"), Some((3, 3)));
        assert!(report.is_position_balanced());
        assert_eq!(report.cases[0].arms[0].arm, "single-fast");
        assert_eq!(report.cases[1].arms[0].arm, "subagent-fast");
    }
}
