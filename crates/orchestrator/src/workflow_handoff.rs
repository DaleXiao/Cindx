use super::{
    WorkflowExecutionCheckpoint, WorkflowOutputKind, WorkflowStepStatus, WorkflowVerificationState,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const WORKFLOW_EXECUTION_HANDOFF_SCHEMA: &str = "cindx.workflow-handoff.v1";

const HANDOFF_OBJECTIVE_MAX_CHARS: usize = 4_000;
const HANDOFF_SUBTASK_MAX_CHARS: usize = 2_000;
const HANDOFF_ERROR_MAX_CHARS: usize = 500;
const HANDOFF_DIGEST_PREFIX_CHARS: usize = 16;
const HANDOFF_MAX_UNRESOLVED_ACTIONS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowExecutionHandoff {
    pub schema: String,
    pub workflow_id: String,
    pub objective: String,
    pub effort: String,
    pub policy: String,
    pub finalized: bool,
    pub continuations: usize,
    pub steps: Vec<WorkflowStepHandoff>,
    pub unresolved_actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStepHandoff {
    pub id: String,
    pub role: String,
    pub model: String,
    pub subtask: String,
    pub status: WorkflowStepStatus,
    pub input_steps: Vec<String>,
    pub output_kind: WorkflowOutputKind,
    pub evidence_count: usize,
    pub verification: WorkflowVerificationState,
    pub completion_satisfied: bool,
    pub output_digest_prefix: String,
    pub input_digest_prefixes: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_summary: Option<String>,
}

impl WorkflowExecutionCheckpoint {
    pub fn execution_handoff(&self) -> WorkflowExecutionHandoff {
        let steps = self
            .plan
            .steps
            .iter()
            .filter_map(|plan_step| {
                let checkpoint = self.steps.get(&plan_step.id)?;
                Some(WorkflowStepHandoff {
                    id: plan_step.id.clone(),
                    role: plan_step.role.clone(),
                    model: checkpoint.model.clone(),
                    subtask: bounded_chars(&plan_step.subtask, HANDOFF_SUBTASK_MAX_CHARS),
                    status: checkpoint.status.clone(),
                    input_steps: plan_step.contract.input_steps.clone(),
                    output_kind: checkpoint.semantic.output_kind.clone(),
                    evidence_count: checkpoint
                        .semantic
                        .evidence_count
                        .max(checkpoint.evidence_count),
                    verification: checkpoint.semantic.verification.clone(),
                    completion_satisfied: checkpoint.semantic.completion_satisfied,
                    output_digest_prefix: digest_prefix(&checkpoint.semantic.output_digest),
                    input_digest_prefixes: checkpoint
                        .semantic
                        .input_digests
                        .iter()
                        .map(|(step_id, digest)| (step_id.clone(), digest_prefix(digest)))
                        .collect(),
                    error_summary: checkpoint
                        .error
                        .as_deref()
                        .map(|error| bounded_chars(error, HANDOFF_ERROR_MAX_CHARS)),
                })
            })
            .collect::<Vec<_>>();

        let mut unresolved_actions = steps
            .iter()
            .filter_map(unresolved_step_action)
            .take(HANDOFF_MAX_UNRESOLVED_ACTIONS)
            .collect::<Vec<_>>();
        if !self.finalized && unresolved_actions.len() < HANDOFF_MAX_UNRESOLVED_ACTIONS {
            unresolved_actions.push(
                "Finalize the workflow only after all required evidence and verification obligations are satisfied."
                    .to_string(),
            );
        }

        WorkflowExecutionHandoff {
            schema: WORKFLOW_EXECUTION_HANDOFF_SCHEMA.to_string(),
            workflow_id: self.plan.workflow_id.clone(),
            objective: bounded_chars(&self.plan.objective, HANDOFF_OBJECTIVE_MAX_CHARS),
            effort: self.plan.effort.clone(),
            policy: self.plan.policy.clone(),
            finalized: self.finalized,
            continuations: self.continuations,
            steps,
            unresolved_actions,
        }
    }

    pub fn execution_handoff_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.execution_handoff())
            .map_err(|error| format!("workflow handoff serialization failed: {error}"))
    }
}

fn unresolved_step_action(step: &WorkflowStepHandoff) -> Option<String> {
    match step.status {
        WorkflowStepStatus::Completed if step.completion_satisfied => None,
        WorkflowStepStatus::Completed => Some(format!(
            "Revalidate step {} because its semantic completion contract is not satisfied.",
            step.id
        )),
        WorkflowStepStatus::Degraded => Some(format!(
            "Independently verify or redo degraded step {} before relying on it.",
            step.id
        )),
        WorkflowStepStatus::Failed => Some(format!(
            "Recover failed step {} or use a verified alternative.",
            step.id
        )),
        WorkflowStepStatus::Running | WorkflowStepStatus::Pending => Some(format!(
            "Complete step {} and satisfy its evidence and verification contract.",
            step.id
        )),
    }
}

fn digest_prefix(value: &str) -> String {
    value.chars().take(HANDOFF_DIGEST_PREFIX_CHARS).collect()
}

fn bounded_chars(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        WorkflowBudget, WorkflowCompletionCriteria, WorkflowPlanIr, WorkflowPlanStep,
        WorkflowStepContract, WorkflowToolPolicy, WORKFLOW_IR_SCHEMA,
    };

    fn checkpoint() -> WorkflowExecutionCheckpoint {
        WorkflowExecutionCheckpoint::new(
            "resume-key",
            WorkflowPlanIr {
                schema: WORKFLOW_IR_SCHEMA.to_string(),
                workflow_id: "workflow-1".to_string(),
                objective: "Build and verify a useful artifact".to_string(),
                effort: "pro".to_string(),
                policy: "best_of_n".to_string(),
                coordinator_model: "conductor".to_string(),
                prompt_profile: "profile-1".to_string(),
                steps: vec![WorkflowPlanStep {
                    id: "research".to_string(),
                    role: "worker".to_string(),
                    model: "model-a".to_string(),
                    subtask: "Gather grounded evidence".to_string(),
                    access: Vec::new(),
                    tool_policy: WorkflowToolPolicy::ReadOnlyEvidence,
                    contract: WorkflowStepContract {
                        input_steps: Vec::new(),
                        output_kind: WorkflowOutputKind::Evidence,
                        completion: WorkflowCompletionCriteria::default(),
                    },
                }],
                budget: WorkflowBudget {
                    max_steps: 1,
                    max_models: 1,
                    max_model_turns_per_step: 2,
                    max_tool_calls_per_step: 2,
                    max_output_tokens_per_step: 2_000,
                },
            },
            1,
        )
    }

    #[test]
    fn handoff_contains_semantics_but_never_raw_worker_output() {
        let mut checkpoint = checkpoint();
        checkpoint
            .complete_step(
                "research",
                "model-a",
                "TOP SECRET RAW WORKER PROSE".to_string(),
                r#"[{"tool":"file.read"}]"#.to_string(),
                2,
            )
            .unwrap();

        let json = checkpoint.execution_handoff_json().unwrap();
        assert!(!json.contains("TOP SECRET RAW WORKER PROSE"));
        assert!(json.contains("output_digest_prefix"));
        assert!(json.contains("evidence_count"));
    }

    #[test]
    fn handoff_exposes_unresolved_contract_actions() {
        let checkpoint = checkpoint();
        let handoff = checkpoint.execution_handoff();

        assert_eq!(handoff.schema, WORKFLOW_EXECUTION_HANDOFF_SCHEMA);
        assert!(handoff
            .unresolved_actions
            .iter()
            .any(|action| action.contains("Complete step research")));
        assert!(handoff
            .unresolved_actions
            .iter()
            .any(|action| action.contains("Finalize the workflow")));
    }

    #[test]
    fn handoff_bounds_objective_and_digest_payloads() {
        let mut checkpoint = checkpoint();
        checkpoint.plan.objective = "o".repeat(HANDOFF_OBJECTIVE_MAX_CHARS + 100);
        checkpoint
            .steps
            .get_mut("research")
            .unwrap()
            .semantic
            .output_digest = "d".repeat(128);

        let handoff = checkpoint.execution_handoff();
        assert!(handoff.objective.chars().count() <= HANDOFF_OBJECTIVE_MAX_CHARS + 12);
        assert_eq!(
            handoff.steps[0].output_digest_prefix.chars().count(),
            HANDOFF_DIGEST_PREFIX_CHARS
        );
    }
}
