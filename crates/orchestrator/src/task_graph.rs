use crate::{WorkflowExecutionCheckpoint, WorkflowStepStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowStepDisposition {
    Resolved,
    ResumeRunning,
    StartAttempt,
    Exhausted,
    Blocked,
}

impl WorkflowExecutionCheckpoint {
    pub fn step_disposition(
        &self,
        step_id: &str,
        attempt_limit: usize,
    ) -> Result<WorkflowStepDisposition, String> {
        let checkpoint = self
            .steps
            .get(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        if matches!(
            checkpoint.status,
            WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
        ) {
            return Ok(WorkflowStepDisposition::Resolved);
        }

        let plan_step = self
            .plan
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .ok_or_else(|| format!("workflow plan is missing step: {step_id}"))?;
        if plan_step.access.iter().any(|dependency| {
            self.steps.get(dependency).is_none_or(|step| {
                !matches!(
                    step.status,
                    WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
                )
            })
        }) {
            return Ok(WorkflowStepDisposition::Blocked);
        }
        if checkpoint.status == WorkflowStepStatus::Running {
            return Ok(WorkflowStepDisposition::ResumeRunning);
        }
        if checkpoint.attempts >= attempt_limit.max(1) {
            return Ok(WorkflowStepDisposition::Exhausted);
        }
        Ok(WorkflowStepDisposition::StartAttempt)
    }

    pub fn prepare_step_attempt(
        &mut self,
        step_id: &str,
        model: &str,
        attempt_limit: usize,
        now_ms: u64,
    ) -> Result<bool, String> {
        match self.step_disposition(step_id, attempt_limit)? {
            WorkflowStepDisposition::Resolved | WorkflowStepDisposition::ResumeRunning => Ok(false),
            WorkflowStepDisposition::StartAttempt => {
                self.begin_step_with_attempt_limit(step_id, model, attempt_limit, now_ms)?;
                Ok(true)
            }
            WorkflowStepDisposition::Exhausted => Err(format!(
                "workflow step {step_id} exhausted its {}-turn budget",
                attempt_limit.max(1)
            )),
            WorkflowStepDisposition::Blocked => Err(format!(
                "workflow step {step_id} is blocked by an incomplete dependency"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        WorkflowBudget, WorkflowPlanIr, WorkflowPlanStep, WorkflowStepContract,
        WorkflowToolPolicy, WORKFLOW_IR_SCHEMA,
    };

    fn checkpoint() -> WorkflowExecutionCheckpoint {
        WorkflowExecutionCheckpoint::new(
            "resume",
            WorkflowPlanIr {
                schema: WORKFLOW_IR_SCHEMA.to_string(),
                workflow_id: "workflow".to_string(),
                objective: "objective".to_string(),
                effort: "auto".to_string(),
                policy: "adaptive".to_string(),
                coordinator_model: "planner".to_string(),
                prompt_profile: "baseline".to_string(),
                steps: vec![
                    WorkflowPlanStep {
                        id: "inspect".to_string(),
                        role: "worker".to_string(),
                        model: "worker".to_string(),
                        subtask: "inspect".to_string(),
                        access: Vec::new(),
                        tool_policy: WorkflowToolPolicy::ReadOnlyEvidence,
                        contract: WorkflowStepContract::inferred(
                            "worker",
                            &[],
                            &WorkflowToolPolicy::ReadOnlyEvidence,
                        ),
                    },
                    WorkflowPlanStep {
                        id: "synthesize".to_string(),
                        role: "synthesizer".to_string(),
                        model: "planner".to_string(),
                        subtask: "synthesize".to_string(),
                        access: vec!["inspect".to_string()],
                        tool_policy: WorkflowToolPolicy::None,
                        contract: WorkflowStepContract::inferred(
                            "synthesizer",
                            &["inspect".to_string()],
                            &WorkflowToolPolicy::None,
                        ),
                    },
                ],
                budget: WorkflowBudget {
                    max_steps: 2,
                    max_models: 2,
                    max_model_turns_per_step: 2,
                    max_tool_calls_per_step: 2,
                    max_output_tokens_per_step: 1_000,
                },
            },
            1,
        )
    }

    #[test]
    fn disposition_distinguishes_resume_blocked_and_exhausted_steps() {
        let mut checkpoint = checkpoint();
        assert_eq!(
            checkpoint.step_disposition("inspect", 2).unwrap(),
            WorkflowStepDisposition::StartAttempt
        );
        assert_eq!(
            checkpoint.step_disposition("synthesize", 2).unwrap(),
            WorkflowStepDisposition::Blocked
        );

        assert!(checkpoint
            .prepare_step_attempt("inspect", "worker", 2, 2)
            .unwrap());
        assert_eq!(
            checkpoint.step_disposition("inspect", 2).unwrap(),
            WorkflowStepDisposition::ResumeRunning
        );
        assert!(!checkpoint
            .prepare_step_attempt("inspect", "worker", 2, 3)
            .unwrap());
        assert_eq!(checkpoint.steps["inspect"].attempts, 1);

        checkpoint.fail_step("inspect", "retry", 4).unwrap();
        assert!(checkpoint
            .prepare_step_attempt("inspect", "worker", 2, 5)
            .unwrap());
        checkpoint.fail_step("inspect", "failed", 6).unwrap();
        assert_eq!(
            checkpoint.step_disposition("inspect", 2).unwrap(),
            WorkflowStepDisposition::Exhausted
        );
    }

    #[test]
    fn completion_contract_records_stable_semantic_lineage() {
        let mut checkpoint = checkpoint();
        let blocked = checkpoint
            .complete_step(
                "synthesize",
                "planner",
                "premature".to_string(),
                "[]".to_string(),
                2,
            )
            .expect_err("dependent step must not complete before its input");
        assert!(blocked.contains("before input inspect"));

        checkpoint
            .complete_step(
                "inspect",
                "worker",
                "evidence result".to_string(),
                r#"[{"source":"workspace"}]"#.to_string(),
                3,
            )
            .unwrap();
        let inspect = &checkpoint.steps["inspect"];
        assert!(inspect.semantic.completion_satisfied);
        assert_eq!(inspect.semantic.evidence_count, 1);
        assert_eq!(inspect.semantic.output_digest.len(), 64);
        let inspect_digest = inspect.semantic.output_digest.clone();

        checkpoint
            .complete_step(
                "synthesize",
                "planner",
                "final result".to_string(),
                "[]".to_string(),
                4,
            )
            .unwrap();
        let synthesize = &checkpoint.steps["synthesize"];
        assert_eq!(
            synthesize.semantic.input_digests.get("inspect"),
            Some(&inspect_digest)
        );
        assert_eq!(
            synthesize.semantic.output_kind,
            crate::WorkflowOutputKind::Synthesis
        );
    }

    #[test]
    fn legacy_v1_checkpoint_recovers_contracts_and_semantic_state() {
        let mut checkpoint = checkpoint();
        checkpoint
            .complete_step(
                "inspect",
                "worker",
                "legacy output".to_string(),
                "[]".to_string(),
                2,
            )
            .unwrap();
        let mut value = serde_json::to_value(&checkpoint).unwrap();
        for step in value["plan"]["steps"].as_array_mut().unwrap() {
            step.as_object_mut().unwrap().remove("contract");
        }
        for step in value["steps"].as_object_mut().unwrap().values_mut() {
            step.as_object_mut().unwrap().remove("semantic");
        }
        let restored = WorkflowExecutionCheckpoint::from_json(
            &serde_json::to_string(&value).unwrap(),
            &["worker".to_string(), "planner".to_string()],
        )
        .unwrap();

        assert_eq!(
            restored.plan.steps[1].contract.input_steps,
            vec!["inspect".to_string()]
        );
        assert!(restored.steps["inspect"].semantic.completion_satisfied);
        assert_eq!(restored.steps["inspect"].semantic.output_digest.len(), 64);
    }
}
