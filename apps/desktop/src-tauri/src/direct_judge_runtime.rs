#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct DirectJudgePlan {
    pub(crate) judge_model: String,
    pub(crate) prompt: String,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_direct_judge(
    effort: &str,
    verification_required: bool,
    executor_model: &str,
    reviewer_model: Option<&str>,
    objective: &str,
    candidate_answer: &str,
) -> Option<DirectJudgePlan> {
    if !orchestrator::direct_judge_eligible(effort, verification_required) {
        return None;
    }
    if candidate_answer.trim().is_empty() {
        return None;
    }
    let judge_model = orchestrator::direct_judge_model(executor_model, reviewer_model)?;
    Some(DirectJudgePlan {
        judge_model: judge_model.to_string(),
        prompt: orchestrator::direct_judge_prompt(objective, candidate_answer),
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn resolve_direct_judge_output(
    output: &str,
) -> Result<orchestrator::DirectJudgeReceipt, String> {
    let receipt = orchestrator::DirectJudgeReceipt::from_judge_output(output)
        .ok_or_else(|| "direct judge returned no receipt".to_string())?;
    receipt.validate()?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_judge_plan_opens_only_for_eligible_distinct_reviewers() {
        let plan = plan_direct_judge(
            "auto",
            true,
            "executor-model",
            Some("reviewer-model"),
            "objective",
            "candidate answer",
        )
        .expect("eligible auto run should plan a judge");
        assert_eq!(plan.judge_model, "reviewer-model");
        assert!(plan.prompt.contains("objective"));
        assert!(plan.prompt.contains("candidate answer"));

        assert!(plan_direct_judge("fast", true, "m", Some("r"), "o", "a").is_none());
        assert!(plan_direct_judge("auto", false, "m", Some("r"), "o", "a").is_none());
        assert!(plan_direct_judge("auto", true, "same", Some("same"), "o", "a").is_none());
        assert!(plan_direct_judge("auto", true, "m", None, "o", "a").is_none());
        assert!(plan_direct_judge("auto", true, "m", Some("r"), "o", "   ").is_none());
    }

    #[test]
    fn direct_judge_output_resolution_enforces_the_receipt_contract() {
        let line = format!(
            "CINDX_DIRECT_JUDGE: {{\"schema\":\"{}\",\"verdict\":\"revise\",\"findings\":[\"gap\"]}}",
            orchestrator::DIRECT_JUDGE_RECEIPT_SCHEMA
        );
        let receipt = resolve_direct_judge_output(&format!("audit\n{line}")).unwrap();
        assert_eq!(receipt.verdict, orchestrator::DirectJudgeVerdict::Revise);

        assert!(resolve_direct_judge_output("no receipt here").is_err());
        let passing_with_findings = format!(
            "CINDX_DIRECT_JUDGE: {{\"schema\":\"{}\",\"verdict\":\"pass\",\"findings\":[\"x\"]}}",
            orchestrator::DIRECT_JUDGE_RECEIPT_SCHEMA
        );
        assert!(resolve_direct_judge_output(&passing_with_findings).is_err());
    }
}
