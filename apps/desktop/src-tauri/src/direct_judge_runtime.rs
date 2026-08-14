use super::*;
use crate::agent_finalizer_runtime::GroundedFinalizerCandidate;

pub(crate) struct DirectJudgePlan {
    pub(crate) judge_model: String,
    pub(crate) prompt: String,
}

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

pub(crate) fn resolve_direct_judge_output(
    output: &str,
) -> Result<orchestrator::DirectJudgeReceipt, String> {
    let receipt = orchestrator::DirectJudgeReceipt::from_judge_output(output)
        .ok_or_else(|| "direct judge returned no receipt".to_string())?;
    receipt.validate()?;
    Ok(receipt)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn effort_from_run_context(run_context: &Metadata) -> String {
    run_context
        .get("agent_effort")
        .cloned()
        .unwrap_or_else(|| "auto".to_string())
}

fn verification_required_from_run_context(run_context: &Metadata) -> bool {
    run_context
        .get("conductor_contract")
        .and_then(|contract| serde_json::from_str::<serde_json::Value>(contract).ok())
        .and_then(|value| value.get("verification_required").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn direct_judge_attribution(role: ModelRole) -> AgentModelAttribution {
    match role {
        ModelRole::Reviewer => AgentModelAttribution::actor(
            AgentActor::IndependentVerifier,
            AgentStage::Verify,
            AgentModelProfile::Verifier,
            AgentEffectAuthority::None,
        ),
        _ => AgentModelAttribution::actor(
            AgentActor::Owner,
            AgentStage::Finalize,
            AgentModelProfile::Primary,
            AgentEffectAuthority::None,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch_direct_judge_call(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    model: &str,
    role: ModelRole,
    stage: &str,
    prompt: String,
) -> Result<String, String> {
    let attribution = direct_judge_attribution(role.clone());
    run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        "direct-judge",
        stage,
        role,
        model,
        prompt,
        attribution,
    )
}

fn ground_repaired_answer(
    runtime: &agent_runtime::AgentLoopState,
    run_context: &Metadata,
    content: &str,
    visible_evidence_sequences: &[u64],
) -> Option<agent_runtime::GroundedCompletionReceipt> {
    let steer_epoch = run_context_steer_epoch(run_context);
    runtime
        .task_contract
        .grounded_completion_receipt(
            steer_epoch,
            runtime.turn,
            content,
            visible_evidence_sequences,
        )
        .ok()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_direct_judge_gate(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    runtime: &agent_runtime::AgentLoopState,
    executor_model: &str,
    objective: &str,
    candidate: GroundedFinalizerCandidate,
) -> (GroundedFinalizerCandidate, String) {
    let plan = plan_direct_judge(
        &effort_from_run_context(run_context),
        verification_required_from_run_context(run_context),
        executor_model,
        Some(config.model_for_role(&ModelRole::Reviewer).as_str()),
        objective,
        &candidate.content,
    );
    let Some(plan) = plan else {
        return (candidate, "direct_judge_not_eligible".to_string());
    };

    let judge_output = match dispatch_direct_judge_call(
        state,
        config,
        task_id,
        run_context,
        &plan.judge_model,
        ModelRole::Reviewer,
        "direct_judge",
        plan.prompt,
    ) {
        Ok(output) => output,
        Err(_) => return (candidate, "direct_judge_unavailable".to_string()),
    };
    let receipt = match resolve_direct_judge_output(&judge_output) {
        Ok(receipt) => receipt,
        Err(_) => return (candidate, "direct_judge_inconclusive".to_string()),
    };
    if receipt.verdict == orchestrator::DirectJudgeVerdict::Pass {
        return (candidate, "direct_judge_passed".to_string());
    }

    let repair_prompt = format!(
        "The following candidate answer was produced for this objective but requires revision before delivery. Return ONLY the complete corrected answer text, nothing else.\n\nObjective:\n{}\n\nCandidate answer:\n{}\n\n{}",
        objective,
        candidate.content,
        orchestrator::direct_judge_repair_directive(&receipt)
    );
    let repaired_output = match dispatch_direct_judge_call(
        state,
        config,
        task_id,
        run_context,
        executor_model,
        ModelRole::Executor,
        "direct_judge_repair",
        repair_prompt,
    ) {
        Ok(output) => sanitize_assistant_content(&output),
        Err(_) => return (candidate, "direct_judge_repair_unavailable".to_string()),
    };
    if repaired_output.trim().is_empty() {
        return (candidate, "direct_judge_repair_empty".to_string());
    }
    let Some(repaired_receipt) = ground_repaired_answer(
        runtime,
        run_context,
        &repaired_output,
        &candidate.receipt.visible_evidence_sequences.clone(),
    ) else {
        return (candidate, "direct_judge_repair_ungrounded".to_string());
    };

    let recheck_prompt = orchestrator::direct_judge_prompt(objective, &repaired_output);
    let disposition = match dispatch_direct_judge_call(
        state,
        config,
        task_id,
        run_context,
        &plan.judge_model,
        ModelRole::Reviewer,
        "direct_judge_recheck",
        recheck_prompt,
    )
    .ok()
    .and_then(|output| resolve_direct_judge_output(&output).ok())
    .map(|recheck| match recheck.verdict {
        orchestrator::DirectJudgeVerdict::Pass => "direct_judge_recheck_passed".to_string(),
        orchestrator::DirectJudgeVerdict::Revise => "direct_judge_recheck_exhausted".to_string(),
    }) {
        Some(disposition) => disposition,
        None => "direct_judge_recheck_inconclusive".to_string(),
    };
    (
        GroundedFinalizerCandidate {
            content: repaired_output,
            receipt: repaired_receipt,
            already_persisted: false,
        },
        disposition,
    )
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
    fn direct_judge_context_parsing_reads_effort_and_verification_flag() {
        let mut context = Metadata::new();
        context.insert("agent_effort".to_string(), "auto".to_string());
        context.insert(
            "conductor_contract".to_string(),
            "{\"verification_required\":true}".to_string(),
        );
        assert_eq!(effort_from_run_context(&context), "auto");
        assert!(verification_required_from_run_context(&context));

        let empty = Metadata::new();
        assert_eq!(effort_from_run_context(&empty), "auto");
        assert!(!verification_required_from_run_context(&empty));

        let mut fast = Metadata::new();
        fast.insert("agent_effort".to_string(), "fast".to_string());
        fast.insert("conductor_contract".to_string(), "not json".to_string());
        assert_eq!(effort_from_run_context(&fast), "fast");
        assert!(!verification_required_from_run_context(&fast));

        let mut no_flag = Metadata::new();
        no_flag.insert(
            "conductor_contract".to_string(),
            "{\"verification_required\":false}".to_string(),
        );
        assert!(!verification_required_from_run_context(&no_flag));
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
