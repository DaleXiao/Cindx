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
        WorkflowBudget, WorkflowPlanIr, WorkflowPlanStep, WorkflowToolPolicy, WORKFLOW_IR_SCHEMA,
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
                    },
                    WorkflowPlanStep {
                        id: "synthesize".to_string(),
                        role: "synthesizer".to_string(),
                        model: "planner".to_string(),
                        subtask: "synthesize".to_string(),
                        access: vec!["inspect".to_string()],
                        tool_policy: WorkflowToolPolicy::None,
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
}
