use serde::{Deserialize, Serialize};

use crate::AgentPolicy;

pub const DIRECT_JUDGE_RECEIPT_SCHEMA: &str = "cindx.direct-judge.v1";
pub const DIRECT_JUDGE_MAX_REPAIR_ROUNDS: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectJudgeVerdict {
    Pass,
    Revise,
}

/// How the reviewer classified one cited location after reading its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectJudgeClaimStatus {
    Entailed,
    Unsupported,
    Contradicted,
}

/// One cited location the reviewer classified, with the reason in its own words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectJudgeClaim {
    pub citation: String,
    pub status: DirectJudgeClaimStatus,
    #[serde(default)]
    pub summary: String,
}

/// Bounds on the optional typed block: the review is one call, not a report.
pub const DIRECT_JUDGE_MAX_CLAIMS: usize = 8;
pub const DIRECT_JUDGE_MAX_CLAIM_SUMMARY_BYTES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectJudgeReceipt {
    pub schema: String,
    pub verdict: DirectJudgeVerdict,
    #[serde(default)]
    pub findings: Vec<String>,
    /// Optional typed classification of the cited locations whose content the
    /// review carried. Absent when the answer cites nothing or the reviewer
    /// omitted it; either way the receipt stays valid, so a stricter contract can
    /// never raise the inconclusive rate.
    #[serde(default)]
    pub claims: Vec<DirectJudgeClaim>,
}

impl DirectJudgeReceipt {
    pub fn from_judge_output(output: &str) -> Option<Self> {
        const MARKER: &str = "CINDX_DIRECT_JUDGE:";
        let mut lines = output.lines().filter(|line| !line.trim().is_empty());
        let receipt_line = lines.next_back()?.trim();
        if lines.any(|line| line.trim().starts_with(MARKER)) {
            return None;
        }
        let payload = receipt_line.strip_prefix(MARKER)?.trim();
        serde_json::from_str(payload).ok()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != DIRECT_JUDGE_RECEIPT_SCHEMA {
            return Err("direct judge receipt schema is unsupported".to_string());
        }
        if self.verdict == DirectJudgeVerdict::Pass
            && self
                .findings
                .iter()
                .any(|finding| !finding.trim().is_empty())
        {
            return Err("a passing direct judge receipt cannot retain findings".to_string());
        }
        if self.claims.len() > DIRECT_JUDGE_MAX_CLAIMS {
            return Err("a direct judge receipt carries too many claims".to_string());
        }
        for claim in &self.claims {
            if claim.citation.trim().is_empty()
                || claim.summary.len() > DIRECT_JUDGE_MAX_CLAIM_SUMMARY_BYTES
            {
                return Err(
                    "a direct judge claim must name its citation and stay bounded".to_string(),
                );
            }
        }
        if self.verdict == DirectJudgeVerdict::Pass
            && self
                .claims
                .iter()
                .any(|claim| claim.status != DirectJudgeClaimStatus::Entailed)
        {
            return Err("a passing direct judge receipt cannot retain refuted claims".to_string());
        }
        Ok(())
    }
}

pub fn direct_judge_model<'a>(
    executor_model: &str,
    reviewer_model: Option<&'a str>,
) -> Option<&'a str> {
    reviewer_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .filter(|model| !model.eq_ignore_ascii_case(executor_model.trim()))
}

pub fn direct_judge_eligible(effort: &str, verification_required: bool) -> bool {
    verification_required
        && crate::AgentPolicy::parse_persisted(effort.trim())
            .is_some_and(|policy| !matches!(policy, AgentPolicy::Fast))
}

pub fn direct_judge_prompt(objective: &str, candidate_answer: &str) -> String {
    format!(
        "You are an independent delivery judge for a Cindx direct-execution run. Audit the candidate answer against the objective only; you cannot use tools and must not invent new requirements. End with exactly one single-line receipt using this shape: CINDX_DIRECT_JUDGE: {{\"schema\":\"{}\",\"verdict\":\"pass|revise\",\"findings\":[],\"claims\":[]}}. Use revise only for concrete defects that block delivery; list each defect as one short actionable finding. When the prompt carried cited workspace content, add one claims entry per cited location as {{\"citation\":\"path:line\",\"status\":\"entailed|unsupported|contradicted\",\"summary\":\"one short reason\"}}, classifying whether what the answer says about that location follows from the quoted lines; omit claims entirely when the answer cites nothing.",
        DIRECT_JUDGE_RECEIPT_SCHEMA
    )
    + &format!("\n\nObjective:\n{objective}\n\nCandidate answer:\n{candidate_answer}")
}

pub fn direct_judge_repair_directive(receipt: &DirectJudgeReceipt) -> String {
    let findings = if receipt.findings.is_empty() {
        "(the judge recorded no findings)".to_string()
    } else {
        receipt.findings.join("; ")
    };
    format!(
        "An independent delivery judge required revisions before delivery. Address every finding below while preserving the rest of the answer.\nFindings:\n{findings}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt_line(verdict: &str, findings: &str) -> String {
        format!(
            "CINDX_DIRECT_JUDGE: {{\"schema\":\"{}\",\"verdict\":\"{}\",\"findings\":{findings}}}",
            DIRECT_JUDGE_RECEIPT_SCHEMA, verdict
        )
    }

    #[test]
    fn direct_judge_receipt_is_exactly_one_final_line() {
        let line = receipt_line("pass", "[]");
        assert!(
            DirectJudgeReceipt::from_judge_output(&format!("audit complete\n{line}\n")).is_some()
        );
        assert!(
            DirectJudgeReceipt::from_judge_output(&format!("{line}\ntrailing prose")).is_none()
        );
        assert!(DirectJudgeReceipt::from_judge_output(&format!("{line}\n{line}")).is_none());
    }

    #[test]
    fn direct_judge_receipt_validation_enforces_schema_and_findings_rule() {
        let passing = receipt_line("pass", "[]");
        let parsed = DirectJudgeReceipt::from_judge_output(&passing).unwrap();
        assert_eq!(parsed.validate(), Ok(()));

        let passing_with_findings = receipt_line("pass", "[\"leftover\"]");
        let parsed = DirectJudgeReceipt::from_judge_output(&passing_with_findings).unwrap();
        assert_eq!(
            parsed.validate(),
            Err("a passing direct judge receipt cannot retain findings".to_string())
        );

        let revising = receipt_line("revise", "[\"fix the boundary case\"]");
        let parsed = DirectJudgeReceipt::from_judge_output(&revising).unwrap();
        assert_eq!(parsed.validate(), Ok(()));
        assert_eq!(parsed.verdict, DirectJudgeVerdict::Revise);

        let bad_schema = DirectJudgeReceipt {
            schema: "wrong".to_string(),
            verdict: DirectJudgeVerdict::Pass,
            findings: Vec::new(),
            claims: Vec::new(),
        };
        assert_eq!(
            bad_schema.validate(),
            Err("direct judge receipt schema is unsupported".to_string())
        );
    }

    #[test]
    fn direct_judge_requires_a_distinct_configured_reviewer() {
        assert_eq!(
            direct_judge_model("planner", Some("reviewer")),
            Some("reviewer")
        );
        assert_eq!(direct_judge_model("planner", Some("planner")), None);
        assert_eq!(direct_judge_model("planner", Some("  planner  ")), None);
        assert_eq!(direct_judge_model("planner", Some("")), None);
        assert_eq!(direct_judge_model("planner", None), None);
    }

    #[test]
    fn direct_judge_eligibility_never_opens_for_fast() {
        // Every tier above fast triggers the judge; legacy labels migrate at
        // ingress, and unparseable effort fails closed.
        assert!(direct_judge_eligible("default", true));
        assert!(direct_judge_eligible("high", true));
        assert!(direct_judge_eligible("xhigh", true));
        assert!(direct_judge_eligible("auto", true));
        assert!(direct_judge_eligible("pro", true));
        assert!(!direct_judge_eligible("fast", true));
        assert!(!direct_judge_eligible("default", false));
        assert!(!direct_judge_eligible("", true));
        assert!(!direct_judge_eligible("future", true));
    }

    #[test]
    fn direct_judge_repair_directive_carries_every_finding() {
        let receipt = DirectJudgeReceipt {
            schema: DIRECT_JUDGE_RECEIPT_SCHEMA.to_string(),
            verdict: DirectJudgeVerdict::Revise,
            findings: vec![
                "missing edge case".to_string(),
                "cite the evidence".to_string(),
            ],
            claims: Vec::new(),
        };
        let directive = direct_judge_repair_directive(&receipt);
        assert!(directive.contains("missing edge case"));
        assert!(directive.contains("cite the evidence"));

        let empty = DirectJudgeReceipt {
            schema: DIRECT_JUDGE_RECEIPT_SCHEMA.to_string(),
            verdict: DirectJudgeVerdict::Revise,
            findings: Vec::new(),
            claims: Vec::new(),
        };
        assert!(direct_judge_repair_directive(&empty).contains("no findings"));
    }

    #[test]
    fn direct_judge_prompt_binds_objective_candidate_and_receipt_contract() {
        let prompt = direct_judge_prompt("evaluate 2+2", "the answer is 4");
        assert!(prompt.contains("CINDX_DIRECT_JUDGE:"));
        assert!(prompt.contains(DIRECT_JUDGE_RECEIPT_SCHEMA));
        assert!(prompt.contains("evaluate 2+2"));
        assert!(prompt.contains("the answer is 4"));
    }
}
