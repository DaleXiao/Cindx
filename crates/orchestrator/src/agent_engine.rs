use crate::{
    AnytimeCandidate, AnytimeCandidateKind, AnytimeController, AnytimeControllerConfig,
    AnytimeControllerSnapshot, AnytimeVerdict, ConductorExecutionContract,
    WorkflowExecutionCheckpoint, WorkflowOutputKind, WorkflowPlanStep, WorkflowStepStatus,
    WorkflowVerificationState,
};

pub const DIRECT_ANCHOR_CANDIDATE_ID: &str = "__direct_anchor";

pub fn direct_anchor_response_prompt(objective: &str, relevant_context: &str) -> String {
    let objective = bounded_chars(objective, 12_000);
    let relevant_context = if relevant_context.trim().is_empty() {
        "(none)".to_string()
    } else {
        bounded_chars(relevant_context, 8_000)
    };
    format!(
        "You are Cindx's independent direct-answer branch. Produce the best user-facing answer you can without tools. Preserve the exact objective, constraints, language, and requested format. Answer simple questions directly. For work that genuinely requires files, tools, or external effects, provide the most useful truthful partial result and state exactly what remains unverified; never claim an effect completed. Do not describe yourself as a branch, do not expose internal orchestration, and do not write instructions for another model. Return only the answer.\n\nUser request:\n{objective}\n\nRelevant prior context:\n{relevant_context}"
    )
}

pub fn direct_anchor_response_verdict(deliverable: bool, verified: bool) -> AnytimeVerdict {
    AnytimeVerdict {
        quality_bps: if deliverable { 6_000 } else { 0 },
        confidence_bps: if deliverable { 5_500 } else { 0 },
        constraint_coverage_bps: if deliverable { 6_000 } else { 0 },
        evidence_count: 0,
        safety_violations: 0,
        deliverable,
        verified: deliverable && verified,
        anchor_uplift_bps: None,
    }
}

fn bounded_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let bounded = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{bounded}\n[truncated]")
    } else {
        bounded
    }
}

/// Durable execution state shared by the desktop runtime and evaluation harnesses.
///
/// The checkpoint is the source of truth. Process-local work is reconstructed from it,
/// so a resumed run and an evaluation run apply the same candidate and delivery rules.
#[derive(Debug, Clone)]
pub struct AgentEngineSession {
    checkpoint: WorkflowExecutionCheckpoint,
    anytime: AnytimeController,
}

impl AgentEngineSession {
    pub fn restore(
        contract: &ConductorExecutionContract,
        checkpoint: WorkflowExecutionCheckpoint,
    ) -> Result<Self, String> {
        let anytime = restore_anytime_controller(contract, &checkpoint)?;
        Ok(Self {
            checkpoint,
            anytime,
        })
    }

    pub fn checkpoint(&self) -> &WorkflowExecutionCheckpoint {
        &self.checkpoint
    }

    pub fn checkpoint_mut(&mut self) -> &mut WorkflowExecutionCheckpoint {
        &mut self.checkpoint
    }

    pub fn anytime(&self) -> &AnytimeController {
        &self.anytime
    }

    pub fn anytime_mut(&mut self) -> &mut AnytimeController {
        &mut self.anytime
    }

    pub fn persist(&mut self) -> Result<(), String> {
        persist_anytime_controller(&mut self.checkpoint, &self.anytime)
    }

    pub fn into_parts(self) -> (WorkflowExecutionCheckpoint, AnytimeController) {
        (self.checkpoint, self.anytime)
    }
}

pub fn persist_anytime_controller(
    checkpoint: &mut WorkflowExecutionCheckpoint,
    controller: &AnytimeController,
) -> Result<(), String> {
    checkpoint.anytime_controller_json = serde_json::to_string(&controller.snapshot())
        .map_err(|error| format!("failed to serialize anytime controller: {error}"))?;
    Ok(())
}

fn anytime_step_kind(step: &WorkflowPlanStep, is_delivery: bool) -> AnytimeCandidateKind {
    if is_delivery || step.contract.output_kind == WorkflowOutputKind::Synthesis {
        AnytimeCandidateKind::Synthesis
    } else if step.contract.output_kind == WorkflowOutputKind::Verification {
        AnytimeCandidateKind::Verification
    } else if step.role == "repair" {
        AnytimeCandidateKind::Repair
    } else {
        AnytimeCandidateKind::Workflow
    }
}

fn refresh_snapshot_contract(
    snapshot: &mut AnytimeControllerSnapshot,
    contract: &ConductorExecutionContract,
) {
    // Keep one non-executing slot for a partial handoff synthesized at interruption time.
    snapshot.config.max_candidates = snapshot
        .config
        .max_candidates
        .max(snapshot.candidates.len().saturating_add(1));
    snapshot.config.max_parallelism = contract.max_parallelism.max(1);
    snapshot.config.min_successful_candidates = contract.min_successful_branches.max(1);
    snapshot.config.min_team_uplift_bps = contract.min_team_uplift_bps;
    snapshot.config.min_distinct_contributions = contract.min_distinct_contributions;
    snapshot.config.requires_synthesis = contract.requires_synthesis;
    snapshot.config.verification_required = contract.verification_required;
    snapshot.config.stop_policy = contract.stop_policy;
}

pub fn restore_anytime_controller(
    contract: &ConductorExecutionContract,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> Result<AnytimeController, String> {
    let plan = &checkpoint.plan;
    let final_step_index = plan.steps.len().saturating_sub(1);

    if !checkpoint.anytime_controller_json.trim().is_empty() {
        let mut snapshot =
            serde_json::from_str::<AnytimeControllerSnapshot>(&checkpoint.anytime_controller_json)
                .map_err(|error| format!("anytime controller checkpoint is invalid: {error}"))?;
        refresh_snapshot_contract(&mut snapshot, contract);
        let mut controller = AnytimeController::from_snapshot(snapshot)?;
        for (index, step) in plan.steps.iter().enumerate() {
            if controller.candidate(&step.id).is_none() {
                continue;
            }
            let is_delivery = index == final_step_index;
            controller.annotate_candidate(
                &step.id,
                anytime_step_kind(step, is_delivery),
                Some(step.independent_contribution_key()),
                is_delivery,
            )?;
        }
        return Ok(controller);
    }

    let mut config = AnytimeControllerConfig::from_contract(contract);
    config.max_candidates = config
        .max_candidates
        .max(plan.steps.len().saturating_add(2));
    let mut controller = AnytimeController::new(config);
    controller.register(AnytimeCandidate::direct_anchor(DIRECT_ANCHOR_CANDIDATE_ID))?;

    for (index, step) in plan.steps.iter().enumerate() {
        let is_delivery = index == final_step_index;
        let uplift = contract
            .expected_uplift_bps
            .saturating_add(u16::try_from(index.saturating_mul(250)).unwrap_or(u16::MAX))
            .min(10_000);
        let mut candidate = AnytimeCandidate::workflow(&step.id, step.access.clone(), uplift)
            .with_kind(anytime_step_kind(step, is_delivery))
            .with_contribution_signature(step.independent_contribution_key());
        if !is_delivery {
            candidate = candidate.as_intermediate();
        }
        controller.register(candidate)?;

        let Some(step_checkpoint) = checkpoint.steps.get(&step.id) else {
            continue;
        };
        if is_delivery && !checkpoint.finalized {
            continue;
        }
        match step_checkpoint.status {
            WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded => {
                controller.mark_running(&step.id)?;
                let semantic = &step_checkpoint.semantic;
                let lineage_complete = semantic.completion_satisfied
                    && !semantic.output_digest.trim().is_empty()
                    && semantic.input_digests.len() == step.access.len();
                let verified = semantic.verification == WorkflowVerificationState::Passed;
                let evidence_count = step_checkpoint.evidence_count.max(semantic.evidence_count);
                controller.observe(
                    &step.id,
                    AnytimeVerdict {
                        quality_bps: match (&step_checkpoint.status, verified) {
                            (WorkflowStepStatus::Completed, true) => 7_250,
                            (WorkflowStepStatus::Completed, false) => 6_250,
                            (WorkflowStepStatus::Degraded, _) => 4_500,
                            _ => 0,
                        },
                        confidence_bps: if verified { 7_000 } else { 5_250 },
                        constraint_coverage_bps: if lineage_complete { 6_500 } else { 0 },
                        evidence_count,
                        safety_violations: 0,
                        deliverable: lineage_complete
                            && step_checkpoint
                                .output
                                .as_ref()
                                .is_some_and(|output| !output.trim().is_empty()),
                        verified,
                        anchor_uplift_bps: None,
                    },
                )?;
            }
            WorkflowStepStatus::Failed => {
                controller.mark_running(&step.id)?;
                controller.fail(&step.id)?;
            }
            WorkflowStepStatus::Cancelled => {
                controller.mark_running(&step.id)?;
                controller.cancel(&step.id)?;
            }
            WorkflowStepStatus::Pending | WorkflowStepStatus::Running => {}
        }
    }
    Ok(controller)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AdaptiveWorkflow, AdaptiveWorkflowStep, ConductorFallbackPolicy, ConductorStopPolicy,
        OrchestrationPolicy, TaskClass, WorkflowBudget, WorkflowPlanIr, WorkflowStepSemanticState,
    };

    fn contract() -> ConductorExecutionContract {
        ConductorExecutionContract {
            task_class: TaskClass::General,
            effort: "pro".to_string(),
            policy: OrchestrationPolicy::BestOfN { candidates: 2 },
            expected_uplift_bps: 4_000,
            confidence_bps: 7_000,
            max_parallelism: 2,
            max_workflow_steps: 4,
            min_successful_branches: 2,
            verification_required: true,
            terminal_model_call_reserve: 2,
            stop_policy: ConductorStopPolicy::Quorum,
            fallback_policy: ConductorFallbackPolicy::BestKnownResult,
            min_team_uplift_bps: 250,
            min_distinct_contributions: 2,
            requires_synthesis: true,
        }
    }

    fn checkpoint() -> WorkflowExecutionCheckpoint {
        let workflow = AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "branch".to_string(),
                    role: "worker".to_string(),
                    model: "worker-model".to_string(),
                    subtask: "Solve independently".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "synthesis-model".to_string(),
                    subtask: "Produce the final answer".to_string(),
                    access: vec!["branch".to_string()],
                },
            ],
        };
        let plan = WorkflowPlanIr::from_adaptive(
            "engine-test",
            "Solve the task",
            "pro",
            "best_of_n",
            "conductor",
            &workflow,
            WorkflowBudget {
                max_steps: 5,
                max_models: 2,
                max_model_turns_per_step: 2,
                max_tool_calls_per_step: 0,
                max_output_tokens_per_step: 2_048,
            },
        );
        WorkflowExecutionCheckpoint::new("resume-engine-test", plan, 1)
    }

    #[test]
    fn new_session_registers_anchor_and_only_delivery_is_commit_eligible() {
        let session = AgentEngineSession::restore(&contract(), checkpoint()).unwrap();
        let snapshot = session.anytime().snapshot();

        assert_eq!(snapshot.candidates.len(), 3);
        assert_eq!(
            session
                .anytime()
                .candidate(DIRECT_ANCHOR_CANDIDATE_ID)
                .unwrap()
                .kind,
            AnytimeCandidateKind::DirectAnchor
        );
        assert!(
            !session
                .anytime()
                .candidate("branch")
                .unwrap()
                .commit_eligible
        );
        assert!(
            session
                .anytime()
                .candidate("final")
                .unwrap()
                .commit_eligible
        );
        assert_eq!(
            session.anytime().candidate("final").unwrap().kind,
            AnytimeCandidateKind::Synthesis
        );
    }

    #[test]
    fn direct_anchor_prompt_preserves_objective_and_bounds_context() {
        let prompt = direct_anchor_response_prompt("Keep the exact objective", &"x".repeat(9_000));
        assert!(prompt.contains("Keep the exact objective"));
        assert!(prompt.contains("[truncated]"));
        assert!(prompt.len() < 21_000);
        assert!(prompt.contains("user-facing answer"));
        assert!(prompt.contains("never claim an effect completed"));
    }

    #[test]
    fn direct_anchor_verdict_never_claims_verification_without_a_deliverable() {
        let missing = direct_anchor_response_verdict(false, true);
        assert!(!missing.deliverable);
        assert!(!missing.verified);

        let verified = direct_anchor_response_verdict(true, true);
        assert!(verified.deliverable);
        assert!(verified.verified);
    }

    #[test]
    fn persisted_session_restores_running_work_as_pending_and_refreshes_contract() {
        let mut initial = AgentEngineSession::restore(&contract(), checkpoint()).unwrap();
        initial.anytime_mut().mark_running("branch").unwrap();
        initial.persist().unwrap();
        let (checkpoint, _) = initial.into_parts();

        let mut changed_contract = contract();
        changed_contract.max_parallelism = 1;
        changed_contract.min_successful_branches = 1;
        let restored = AgentEngineSession::restore(&changed_contract, checkpoint).unwrap();
        let snapshot = restored.anytime().snapshot();

        assert_eq!(
            restored.anytime().candidate("branch").unwrap().state,
            crate::AnytimeCandidateState::Pending
        );
        assert_eq!(snapshot.config.max_parallelism, 1);
        assert_eq!(snapshot.config.min_successful_candidates, 1);
        assert!(snapshot.config.max_candidates > snapshot.candidates.len());
    }

    #[test]
    fn checkpoint_semantics_rebuild_a_verified_delivery_candidate() {
        let mut checkpoint = checkpoint();
        checkpoint.finalized = true;
        let step = checkpoint.steps.get_mut("final").unwrap();
        step.status = WorkflowStepStatus::Completed;
        step.output = Some("final answer".to_string());
        step.evidence_count = 2;
        step.semantic = WorkflowStepSemanticState {
            input_digests: [("branch".to_string(), "digest".to_string())]
                .into_iter()
                .collect(),
            output_digest: "final-digest".to_string(),
            evidence_count: 2,
            verification: WorkflowVerificationState::Passed,
            completion_satisfied: true,
            ..WorkflowStepSemanticState::default()
        };

        let session = AgentEngineSession::restore(&contract(), checkpoint).unwrap();
        let best = session.anytime().best().unwrap();
        assert_eq!(best.candidate_id, "final");
        assert!(best.verdict.deliverable);
        assert!(best.verdict.verified);
    }

    #[test]
    fn dual_read_only_roots_fan_out_within_the_two_branch_budget() {
        let workflow = AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "survey_a".to_string(),
                    role: "worker".to_string(),
                    model: "worker-model".to_string(),
                    subtask: "Survey the public module surface".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "survey_b".to_string(),
                    role: "worker".to_string(),
                    model: "worker-model".to_string(),
                    subtask: "Survey the internal test surface".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "verify".to_string(),
                    role: "verifier".to_string(),
                    model: "reviewer-model".to_string(),
                    subtask: "Audit both survey branches".to_string(),
                    access: vec!["survey_a".to_string(), "survey_b".to_string()],
                },
                AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "synthesis-model".to_string(),
                    subtask: "Hand the audited survey to the Owner".to_string(),
                    access: vec!["verify".to_string()],
                },
            ],
        };
        let mut plan = WorkflowPlanIr::from_adaptive(
            "read-only-parallel",
            "Survey the workspace without changes",
            "pro",
            "best_of_n",
            "conductor",
            &workflow,
            WorkflowBudget {
                max_steps: 5,
                max_models: 3,
                max_model_turns_per_step: 2,
                max_tool_calls_per_step: 6,
                max_output_tokens_per_step: 2_048,
            },
        );
        plan.parallel_read_only_specialists = true;
        let verification = plan
            .steps
            .iter_mut()
            .find(|step| step.contract.output_kind == crate::WorkflowOutputKind::Verification)
            .expect("verifier step exists");
        verification.tool_policy = crate::WorkflowToolPolicy::None;
        plan.validate_owner_execution_graph(true).unwrap();
        let checkpoint = WorkflowExecutionCheckpoint::new("fan-out", plan, 1);

        let session = AgentEngineSession::restore(&contract(), checkpoint).unwrap();
        let mut snapshot = session.anytime().snapshot();
        assert_eq!(snapshot.config.max_parallelism, 2);
        // The desktop foreground path drops the legacy direct anchor before
        // scheduling; mirror that so the branch budget is measured alone.
        snapshot
            .candidates
            .retain(|candidate| candidate.id != DIRECT_ANCHOR_CANDIDATE_ID);
        let controller = AnytimeController::from_snapshot(snapshot).unwrap();

        let ready: Vec<String> = controller
            .ready_candidates()
            .into_iter()
            .map(|candidate| candidate.id.clone())
            .collect();
        assert!(ready.iter().any(|id| id == "survey_a"), "{ready:?}");
        assert!(ready.iter().any(|id| id == "survey_b"), "{ready:?}");
        assert!(!ready.iter().any(|id| id == "verify"), "{ready:?}");
    }
}
