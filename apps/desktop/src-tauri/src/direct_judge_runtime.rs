use crate::agent_finalizer_runtime::GroundedFinalizerCandidate;
use crate::app_state::AppState;
use crate::collaboration_stage_runtime::run_collaboration_stage;
use crate::configuration_models::ProviderConfig;
use agent_core::{
    AgentActor, AgentEffectAuthority, AgentModelAttribution, AgentModelProfile, AgentStage,
    Metadata, ModelRole, TaskId,
};
use agent_runtime::{run_context_steer_epoch, sanitize_assistant_content};

pub(crate) struct DirectJudgePlan {
    pub(crate) judge_model: String,
    pub(crate) prompt: String,
}

/// Result of the delivery-judge gate. `Delivered` carries the (possibly
/// repaired) candidate and its recorded disposition; `Blocked` is the
/// fail-closed exit: the run commits the ordinary failure terminal with the
/// "verification failed + findings" message instead of delivering.
pub(crate) enum DirectJudgeGateOutcome {
    Delivered(GroundedFinalizerCandidate, String),
    Blocked {
        disposition: String,
        message: String,
    },
}

// The fail-closed delivery decision now lives in the portable agent-application
// crate (Phase 4 fidelity, audit 3b) so the evaluation harness applies the same
// rule as the product; re-exported here so desktop call sites are unchanged.
pub(crate) use agent_application::direct_judge_fail_closed_block;

pub(crate) fn plan_direct_judge(
    effort: &str,
    verification_required: bool,
    executor_model: &str,
    reviewer_model: Option<&str>,
    objective: &str,
    candidate_answer: &str,
) -> Option<DirectJudgePlan> {
    if !agent_core::direct_judge_eligible(effort, verification_required) {
        return None;
    }
    if candidate_answer.trim().is_empty() {
        return None;
    }
    let judge_model = agent_core::direct_judge_model(executor_model, reviewer_model)?;
    Some(DirectJudgePlan {
        judge_model: judge_model.to_string(),
        prompt: agent_core::direct_judge_prompt(objective, candidate_answer),
    })
}

pub(crate) fn resolve_direct_judge_output(
    output: &str,
) -> Result<agent_core::DirectJudgeReceipt, String> {
    let receipt = agent_core::DirectJudgeReceipt::from_judge_output(output)
        .ok_or_else(|| "direct judge returned no receipt".to_string())?;
    receipt.validate()?;
    Ok(receipt)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn effort_from_run_context(run_context: &Metadata) -> String {
    run_context
        .get("agent_effort")
        .map(String::as_str)
        .and_then(agent_core::AgentPolicy::parse_persisted)
        .map(|policy| policy.label().to_string())
        .unwrap_or_else(|| "default".to_string())
}

fn verification_required_from_run_context(run_context: &Metadata) -> bool {
    run_context
        .get("conductor_contract")
        .and_then(|contract| serde_json::from_str::<serde_json::Value>(contract).ok())
        .and_then(|value| {
            value
                .get("verification_required")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false)
}

fn direct_judge_attribution(role: ModelRole) -> AgentModelAttribution {
    guardian_or_verifier_attribution(role)
}

/// Shared Reviewer/Owner attribution for single-shot non-streaming review
/// calls: the delivery judge and the permission guardian both dispatch through
/// the same collaboration-stage infrastructure.
pub(crate) fn guardian_or_verifier_attribution(role: ModelRole) -> AgentModelAttribution {
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

pub(crate) fn direct_judge_execution_summary(
    runtime: &agent_runtime::AgentLoopState,
    candidate: &str,
) -> String {
    let contract = &runtime.task_contract;
    let mutations = contract.successful_mutations();
    let verification_state = if mutations == 0 {
        "no workspace mutations occurred"
    } else if contract.latest_mutation_verified() {
        "all workspace mutations carry post-mutation verification evidence"
    } else {
        "workspace mutations occurred WITHOUT post-mutation verification"
    };
    let policy = match contract.workspace_verification_policy() {
        agent_runtime::WorkspaceVerificationPolicy::RequiredAfterMutation => {
            "workspace verification is required after mutations"
        }
        _ => "workspace verification is not required",
    };
    let facts = format!(
        "Execution facts (trusted runtime record):\n- Successful workspace mutations: {mutations}\n- Mutation verification: {verification_state}\n- Policy: {policy}\n- Grounding evidence recorded: {}",
        contract.has_prompt_evidence()
    );
    // P1-10: bind the candidate's workspace citations to the locations this run
    // actually observed, so the judge sees claim-evidence facts instead of counts
    // alone. Additive only: an answer that cites nothing produces no block and
    // leaves the prompt byte-identical.
    let observed = agent_runtime::observed_locations_from_messages(&runtime.messages);
    let receipt = agent_runtime::bind_answer_citations(candidate, &observed);
    let citation_facts = agent_runtime::claim_evidence_facts(&receipt);
    if citation_facts.is_empty() {
        facts
    } else {
        format!("{facts}\n{citation_facts}")
    }
}

pub(crate) fn direct_judge_prompt_with_facts(
    objective: &str,
    candidate: &str,
    runtime: &agent_runtime::AgentLoopState,
) -> String {
    format!(
        "{}\n\n{}",
        agent_core::direct_judge_prompt(objective, candidate),
        direct_judge_execution_summary(runtime, candidate)
    )
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
) -> DirectJudgeGateOutcome {
    let plan = plan_direct_judge(
        &effort_from_run_context(run_context),
        verification_required_from_run_context(run_context),
        executor_model,
        Some(config.model_for_role(&ModelRole::Reviewer).as_str()),
        objective,
        &candidate.content,
    );
    let Some(plan) = plan else {
        return DirectJudgeGateOutcome::Delivered(
            candidate,
            "direct_judge_not_eligible".to_string(),
        );
    };

    let judge_output = match dispatch_direct_judge_call(
        state,
        config,
        task_id,
        run_context,
        &plan.judge_model,
        ModelRole::Reviewer,
        "direct_judge",
        format!(
            "{prompt}\n\n{facts}",
            prompt = plan.prompt,
            facts = direct_judge_execution_summary(runtime, &candidate.content)
        ),
    ) {
        Ok(output) => output,
        Err(_) => {
            return DirectJudgeGateOutcome::Delivered(
                candidate,
                "direct_judge_unavailable".to_string(),
            )
        }
    };
    let receipt = match resolve_direct_judge_output(&judge_output) {
        Ok(receipt) => receipt,
        Err(_) => {
            return DirectJudgeGateOutcome::Delivered(
                candidate,
                "direct_judge_inconclusive".to_string(),
            )
        }
    };
    if receipt.verdict == agent_core::DirectJudgeVerdict::Pass {
        return DirectJudgeGateOutcome::Delivered(candidate, "direct_judge_passed".to_string());
    }

    let repair_prompt = format!(
        "The following candidate answer was produced for this objective but requires revision before delivery. Return ONLY the complete corrected answer text, nothing else.\n\nObjective:\n{}\n\nCandidate answer:\n{}\n\n{}",
        objective,
        candidate.content,
        agent_core::direct_judge_repair_directive(&receipt)
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
        Err(_) => {
            return DirectJudgeGateOutcome::Delivered(
                candidate,
                "direct_judge_repair_unavailable".to_string(),
            )
        }
    };
    if repaired_output.trim().is_empty() {
        return DirectJudgeGateOutcome::Delivered(
            candidate,
            "direct_judge_repair_empty".to_string(),
        );
    }
    let Some(repaired_receipt) = ground_repaired_answer(
        runtime,
        run_context,
        &repaired_output,
        &candidate.receipt.visible_evidence_sequences.clone(),
    ) else {
        if let Some(message) = direct_judge_fail_closed_block(
            config.direct_judge_fail_closed,
            runtime.task_contract.successful_mutations(),
            agent_application::DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED,
            &receipt.findings,
        ) {
            return DirectJudgeGateOutcome::Blocked {
                disposition: agent_application::DIRECT_JUDGE_DISPOSITION_FAIL_CLOSED_BLOCKED
                    .to_string(),
                message,
            };
        }
        return DirectJudgeGateOutcome::Delivered(
            candidate,
            "direct_judge_repair_ungrounded".to_string(),
        );
    };

    let recheck_prompt = direct_judge_prompt_with_facts(objective, &repaired_output, runtime);
    let recheck = dispatch_direct_judge_call(
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
    .and_then(|output| resolve_direct_judge_output(&output).ok());
    let repaired_candidate = GroundedFinalizerCandidate {
        content: repaired_output,
        receipt: repaired_receipt,
        already_persisted: false,
    };
    match recheck {
        Some(recheck) if recheck.verdict == agent_core::DirectJudgeVerdict::Pass => {
            DirectJudgeGateOutcome::Delivered(
                repaired_candidate,
                "direct_judge_recheck_passed".to_string(),
            )
        }
        Some(recheck) => {
            let findings = if recheck.findings.is_empty() {
                &receipt.findings
            } else {
                &recheck.findings
            };
            if let Some(message) = direct_judge_fail_closed_block(
                config.direct_judge_fail_closed,
                runtime.task_contract.successful_mutations(),
                agent_application::DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
                findings,
            ) {
                return DirectJudgeGateOutcome::Blocked {
                    disposition: agent_application::DIRECT_JUDGE_DISPOSITION_FAIL_CLOSED_BLOCKED
                        .to_string(),
                    message,
                };
            }
            DirectJudgeGateOutcome::Delivered(
                repaired_candidate,
                "direct_judge_recheck_exhausted".to_string(),
            )
        }
        None => DirectJudgeGateOutcome::Delivered(
            repaired_candidate,
            "direct_judge_recheck_inconclusive".to_string(),
        ),
    }
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
    fn direct_judge_execution_summary_reports_unmutated_state() {
        let runtime = agent_runtime::start_agent_loop(
            agent_core::TaskId("judge-facts".to_string()),
            "objective",
            agent_runtime::AgentRuntimeConfig::default(),
        );
        let summary = direct_judge_execution_summary(&runtime, "candidate");
        assert!(summary.contains("Successful workspace mutations: 0"));
        assert!(summary.contains("no workspace mutations occurred"));
        // An answer with no workspace citation adds no claim-evidence block.
        assert!(!summary.contains("Answer citations"));
        let prompt = direct_judge_prompt_with_facts("objective", "candidate", &runtime);
        assert!(prompt.contains("Execution facts"));
        assert!(prompt.contains("CINDX_DIRECT_JUDGE:"));
    }

    /// The judge's trusted facts include the answer's citations bound to the
    /// locations the run really observed (P1-10), so a fabricated or refuted
    /// citation is visible to the reviewer instead of passing as prose.
    #[test]
    fn direct_judge_execution_summary_binds_answer_citations_to_observed_locations() {
        let mut runtime = agent_runtime::start_agent_loop(
            agent_core::TaskId("judge-claims".to_string()),
            "objective",
            agent_runtime::AgentRuntimeConfig::default(),
        );
        // Two durable tool messages stamped at dispatch: one successful read and
        // one failed read of a path the answer still cites.
        for (input, succeeded) in [
            (r#"{"path":"crates/agent-rag/src/lib.rs"}"#, true),
            (r#"{"path":"src/gone.rs"}"#, false),
        ] {
            let observed =
                agent_runtime::observed_locations_for_call("file.read", input, succeeded);
            let mut metadata = Metadata::new();
            agent_runtime::insert_observed_locations(&mut metadata, &observed);
            runtime.messages.push(agent_core::Message {
                role: agent_core::MessageRole::Tool,
                content: "observation".to_string(),
                metadata,
            });
        }

        let candidate = "The planner is at crates/agent-rag/src/lib.rs:1142, the \
                         missing file was src/gone.rs:9, and see src/never-read.rs:3.";
        let summary = direct_judge_execution_summary(&runtime, candidate);

        assert!(
            summary
                .contains("Answer citations: 3 parsed, 1 supported, 1 unsupported, 1 contradicted"),
            "summary was {summary}"
        );
        assert!(
            summary.contains("src/never-read.rs:3"),
            "summary was {summary}"
        );
        assert!(summary.contains("src/gone.rs:9"), "summary was {summary}");
        assert!(summary.contains("did not succeed"), "summary was {summary}");
        // Contradictions are listed before gaps.
        assert!(
            summary.find("contradicted `src/gone.rs:9`")
                < summary.find("unsupported `src/never-read.rs:3`"),
            "summary was {summary}"
        );

        // The same runtime with an answer that cites nothing is unchanged.
        let plain = direct_judge_execution_summary(&runtime, "Done; every test passes.");
        assert!(!plain.contains("Answer citations"));
        assert!(plain.contains("Execution facts (trusted runtime record)"));
    }

    #[test]
    fn direct_judge_context_parsing_reads_effort_and_verification_flag() {
        let mut context = Metadata::new();
        context.insert("agent_effort".to_string(), "auto".to_string());
        context.insert(
            "conductor_contract".to_string(),
            "{\"verification_required\":true}".to_string(),
        );
        assert_eq!(effort_from_run_context(&context), "default");
        assert!(verification_required_from_run_context(&context));

        let mut canonical = Metadata::new();
        canonical.insert("agent_effort".to_string(), "xhigh".to_string());
        assert_eq!(effort_from_run_context(&canonical), "xhigh");

        let empty = Metadata::new();
        assert_eq!(effort_from_run_context(&empty), "default");
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
    fn direct_judge_fail_closed_defaults_off_and_stays_fail_open() {
        // Default off: even a judged failure on a mutation-bearing run keeps
        // the historical fail-open behavior.
        for disposition in [
            agent_application::DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
            agent_application::DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED,
        ] {
            assert!(
                direct_judge_fail_closed_block(false, 2, disposition, &["gap".to_string()])
                    .is_none(),
                "{disposition} must stay fail-open when the toggle is off"
            );
        }
    }

    #[test]
    fn direct_judge_fail_closed_blocks_failed_recheck_on_mutation_runs() {
        let message = direct_judge_fail_closed_block(
            true,
            1,
            agent_application::DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
            &["missing edge case".to_string()],
        )
        .expect("a failed recheck on a mutation run must block");
        assert!(message.contains("not delivered"));
        assert!(message.contains("missing edge case"));
    }

    #[test]
    fn direct_judge_fail_closed_blocks_ungrounded_repair_on_mutation_runs() {
        let message = direct_judge_fail_closed_block(
            true,
            3,
            agent_application::DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED,
            &[],
        )
        .expect("an ungrounded repair on a mutation run must block");
        assert!(message.contains("not delivered"));
        assert!(message.contains("no findings recorded"));
    }

    #[test]
    fn direct_judge_fail_closed_ignores_mutation_free_runs() {
        for disposition in [
            agent_application::DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
            agent_application::DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED,
        ] {
            assert!(
                direct_judge_fail_closed_block(true, 0, disposition, &["gap".to_string()])
                    .is_none(),
                "{disposition} must stay fail-open without workspace mutations"
            );
        }
    }

    #[test]
    fn direct_judge_fail_closed_keeps_infrastructure_failures_fail_open() {
        for disposition in [
            agent_application::DIRECT_JUDGE_DISPOSITION_UNAVAILABLE,
            agent_application::DIRECT_JUDGE_DISPOSITION_INCONCLUSIVE,
            agent_application::DIRECT_JUDGE_DISPOSITION_RECHECK_INCONCLUSIVE,
            agent_application::DIRECT_JUDGE_DISPOSITION_REPAIR_UNAVAILABLE,
            agent_application::DIRECT_JUDGE_DISPOSITION_REPAIR_EMPTY,
            agent_application::DIRECT_JUDGE_DISPOSITION_PASSED,
            agent_application::DIRECT_JUDGE_DISPOSITION_RECHECK_PASSED,
            agent_application::DIRECT_JUDGE_DISPOSITION_NOT_ELIGIBLE,
            agent_application::DIRECT_JUDGE_DISPOSITION_NOT_APPLICABLE,
        ] {
            assert!(
                direct_judge_fail_closed_block(true, 5, disposition, &["gap".to_string()])
                    .is_none(),
                "{disposition} is not a judged quality failure and must stay fail-open"
            );
        }
    }

    #[test]
    fn direct_judge_output_resolution_enforces_the_receipt_contract() {
        let line = format!(
            "CINDX_DIRECT_JUDGE: {{\"schema\":\"{}\",\"verdict\":\"revise\",\"findings\":[\"gap\"]}}",
            agent_core::DIRECT_JUDGE_RECEIPT_SCHEMA
        );
        let receipt = resolve_direct_judge_output(&format!("audit\n{line}")).unwrap();
        assert_eq!(receipt.verdict, agent_core::DirectJudgeVerdict::Revise);

        assert!(resolve_direct_judge_output("no receipt here").is_err());
        let passing_with_findings = format!(
            "CINDX_DIRECT_JUDGE: {{\"schema\":\"{}\",\"verdict\":\"pass\",\"findings\":[\"x\"]}}",
            agent_core::DIRECT_JUDGE_RECEIPT_SCHEMA
        );
        assert!(resolve_direct_judge_output(&passing_with_findings).is_err());
    }
}
