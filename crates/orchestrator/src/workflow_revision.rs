use crate::{
    WorkflowCompletionCriteria, WorkflowExecutionCheckpoint, WorkflowOutputKind, WorkflowPlanIr,
    WorkflowPlanStep, WorkflowStepCheckpoint, WorkflowStepContract, WorkflowStepSemanticState,
    WorkflowStepStatus, WorkflowToolPolicy,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const WORKFLOW_REVISION_SCHEMA: &str = "cindx.workflow.revision.v1";
pub const MAX_WORKFLOW_PLAN_REVISIONS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowReplacementStep {
    pub id: String,
    pub role: String,
    pub model: String,
    pub subtask: String,
    #[serde(default)]
    pub access: Vec<String>,
    pub tool_policy: WorkflowToolPolicy,
    pub output_kind: WorkflowOutputKind,
}

impl WorkflowReplacementStep {
    fn into_plan_step(self) -> WorkflowPlanStep {
        WorkflowPlanStep {
            id: self.id,
            role: self.role,
            model: self.model,
            subtask: self.subtask,
            contract: WorkflowStepContract {
                input_steps: self.access.clone(),
                output_kind: self.output_kind,
                completion: WorkflowCompletionCriteria::default(),
            },
            access: self.access,
            tool_policy: self.tool_policy,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPlanRevision {
    pub schema: String,
    pub target_step_id: String,
    pub rationale: String,
    pub replacement_steps: Vec<WorkflowReplacementStep>,
}

impl WorkflowPlanRevision {
    pub fn from_json(value: &str) -> Result<Self, String> {
        let start = value
            .find('{')
            .ok_or_else(|| "workflow revision did not contain a JSON object".to_string())?;
        let end = value
            .rfind('}')
            .filter(|end| *end >= start)
            .ok_or_else(|| "workflow revision JSON is incomplete".to_string())?;
        serde_json::from_str(&value[start..=end])
            .map_err(|error| format!("workflow revision JSON is invalid: {error}"))
    }

    pub fn validate_shape(&self) -> Result<(), String> {
        if self.schema != WORKFLOW_REVISION_SCHEMA {
            return Err(format!(
                "unsupported workflow revision schema: {}",
                self.schema
            ));
        }
        if self.target_step_id.trim().is_empty() {
            return Err("workflow revision target is empty".to_string());
        }
        let rationale_chars = self.rationale.trim().chars().count();
        if rationale_chars == 0 || rationale_chars > 2_000 {
            return Err("workflow revision rationale must contain 1..=2000 characters".to_string());
        }
        if !(1..=2).contains(&self.replacement_steps.len()) {
            return Err("workflow revision must provide one or two replacement steps".to_string());
        }
        for step in &self.replacement_steps {
            if step.id.trim().is_empty() || step.id.len() > 80 {
                return Err("workflow replacement step id is invalid".to_string());
            }
            if step.role.trim().is_empty() || step.role.len() > 48 {
                return Err(format!(
                    "workflow replacement step {} role is invalid",
                    step.id
                ));
            }
            if step.subtask.trim().is_empty() || step.subtask.chars().count() > 4_000 {
                return Err(format!(
                    "workflow replacement step {} subtask is invalid",
                    step.id
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPlanRevisionRecord {
    pub revision: usize,
    pub target_step_id: String,
    pub replacement_step_ids: Vec<String>,
    pub rationale: String,
    pub applied_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct WorkflowRevisionRequest {
    pub plan: WorkflowPlanIr,
    pub failed_step_id: String,
    pub failure: String,
    pub preserved_outputs: Vec<String>,
    pub allowed_models: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WorkflowRevisionHarness {
    request: WorkflowRevisionRequest,
}

impl WorkflowRevisionHarness {
    pub fn new(request: WorkflowRevisionRequest) -> Self {
        Self { request }
    }

    pub fn planning_prompt(&self) -> Result<String, String> {
        let failed_step = self
            .request
            .plan
            .steps
            .iter()
            .find(|step| step.id == self.request.failed_step_id)
            .ok_or_else(|| "workflow revision request targets an unknown step".to_string())?;
        let remaining_capacity = self
            .request
            .plan
            .budget
            .max_steps
            .saturating_sub(self.request.plan.steps.len().saturating_sub(1))
            .clamp(1, 2);
        let models = self.request.allowed_models.join("\n- ");
        let preserved = if self.request.preserved_outputs.is_empty() {
            "(none)".to_string()
        } else {
            self.request.preserved_outputs.join("\n")
        };
        let target_is_delivery = self
            .request
            .plan
            .steps
            .last()
            .is_some_and(|step| step.id == failed_step.id);
        Ok(format!(
            "You are repairing a running Cindx task graph after a step failed. Return only one strict JSON object. Replace exactly the failed step with one or at most {remaining_capacity} causally ordered steps. Preserve completed work, use exact model strings from the allowed pool, and do not broaden tool authority. A replacement access list may reference retained earlier step ids or earlier replacement ids only. The last replacement automatically substitutes for the failed step in downstream dependencies. If the target is the delivery step, the last replacement output_kind must be synthesis; otherwise no replacement may use synthesis. tool_policy must be none, read_only_evidence, or read_only_exploration. output_kind must be analysis, evidence, verification, or synthesis. Do not answer the user and do not include markdown.\n\nSchema example:\n{{\"schema\":\"{schema}\",\"target_step_id\":\"{target}\",\"rationale\":\"why the failed decomposition should change\",\"replacement_steps\":[{{\"id\":\"repair\",\"role\":\"domain_specialist\",\"model\":\"{model}\",\"subtask\":\"bounded replacement work\",\"access\":[],\"tool_policy\":\"none\",\"output_kind\":\"analysis\"}}]}}\n\nTarget is delivery: {target_is_delivery}\nFailed step:\n{}\nFailure:\n{}\nPreserved completed outputs:\n{}\nAllowed models:\n- {}\nFull current plan:\n{}",
            serde_json::to_string(failed_step)
                .map_err(|error| format!("failed to serialize workflow step: {error}"))?,
            self.request.failure,
            preserved,
            models,
            self.request.plan.to_json()?,
            schema = WORKFLOW_REVISION_SCHEMA,
            target = failed_step.id,
            model = self
                .request
                .allowed_models
                .first()
                .map(String::as_str)
                .unwrap_or("configured-model"),
        ))
    }

    pub fn repair_prompt(&self, response: &str, error: &str) -> Result<String, String> {
        Ok(format!(
            "The deterministic workflow revision validator rejected the previous response. Correct only the JSON structure and constraints; return JSON and no prose.\n\nValidation error:\n{}\n\nRejected response:\n{}\n\nOriginal request:\n{}",
            error,
            truncate_revision_text(response, 6_000),
            self.planning_prompt()?
        ))
    }

    pub fn parse_revision(&self, response: &str) -> Result<WorkflowPlanRevision, String> {
        let revision = WorkflowPlanRevision::from_json(response)?;
        revision.validate_shape()?;
        if revision.target_step_id != self.request.failed_step_id {
            return Err("workflow revision changed the requested target step".to_string());
        }
        if revision.replacement_steps.iter().any(|step| {
            !self
                .request
                .allowed_models
                .iter()
                .any(|model| model == &step.model)
        }) {
            return Err("workflow revision selected a model outside the allowed pool".to_string());
        }
        if revision.replacement_steps.len() == 1 {
            let failed = self
                .request
                .plan
                .steps
                .iter()
                .find(|step| step.id == self.request.failed_step_id)
                .ok_or_else(|| "workflow revision target is no longer in the plan".to_string())?;
            let replacement = &revision.replacement_steps[0];
            let same_strategy = replacement.role.trim() == failed.role.trim()
                && replacement.model.trim() == failed.model.trim()
                && replacement.subtask.trim() == failed.subtask.trim()
                && replacement.access == failed.access
                && replacement.tool_policy == failed.tool_policy
                && replacement.output_kind == failed.contract.output_kind;
            if same_strategy {
                return Err(
                    "workflow revision repeats the failed strategy without changing its work, model, dependencies, or tool contract"
                        .to_string(),
                );
            }
        }
        Ok(revision)
    }
}

fn truncate_revision_text(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
}

impl WorkflowExecutionCheckpoint {
    pub fn apply_plan_revision(
        &mut self,
        revision: WorkflowPlanRevision,
        allowed_models: &[String],
        now_ms: u64,
    ) -> Result<WorkflowPlanRevisionRecord, String> {
        revision.validate_shape()?;
        if self.plan_revisions.len() >= MAX_WORKFLOW_PLAN_REVISIONS {
            return Err(format!(
                "workflow exhausted its {MAX_WORKFLOW_PLAN_REVISIONS}-revision budget"
            ));
        }
        let target_index = self
            .plan
            .steps
            .iter()
            .position(|step| step.id == revision.target_step_id)
            .ok_or_else(|| {
                format!(
                    "workflow revision targets unknown step {}",
                    revision.target_step_id
                )
            })?;
        let target_checkpoint = self
            .steps
            .get(&revision.target_step_id)
            .ok_or_else(|| "workflow revision target has no checkpoint".to_string())?;
        if matches!(
            target_checkpoint.status,
            WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
        ) {
            return Err("workflow revision cannot replace a resolved step".to_string());
        }
        if self.plan.steps.iter().skip(target_index + 1).any(|step| {
            self.steps.get(&step.id).is_some_and(|checkpoint| {
                matches!(
                    checkpoint.status,
                    WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
                )
            })
        }) {
            return Err("workflow revision cannot rewire already resolved descendants".to_string());
        }

        let replacement_count = revision.replacement_steps.len();
        let revised_step_count = self
            .plan
            .steps
            .len()
            .saturating_sub(1)
            .saturating_add(replacement_count);
        if revised_step_count > self.plan.budget.max_steps {
            return Err(format!(
                "workflow revision would exceed the declared {}-step budget",
                self.plan.budget.max_steps
            ));
        }

        let target_was_delivery = target_index + 1 == self.plan.steps.len();
        let replacement_steps = revision
            .replacement_steps
            .into_iter()
            .map(WorkflowReplacementStep::into_plan_step)
            .collect::<Vec<_>>();
        let replacement_ids = replacement_steps
            .iter()
            .map(|step| step.id.clone())
            .collect::<Vec<_>>();
        let replacement_id_set = replacement_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if replacement_id_set.len() != replacement_ids.len() {
            return Err("workflow revision contains duplicate replacement ids".to_string());
        }
        let retained_ids = self
            .plan
            .steps
            .iter()
            .filter(|step| step.id != revision.target_step_id)
            .map(|step| step.id.as_str())
            .collect::<BTreeSet<_>>();
        if let Some(duplicate) = replacement_id_set
            .iter()
            .find(|step_id| retained_ids.contains(**step_id))
        {
            return Err(format!(
                "workflow replacement id conflicts with retained step {duplicate}"
            ));
        }
        let replacement_terminal = replacement_steps
            .last()
            .ok_or_else(|| "workflow revision has no replacement terminal".to_string())?;
        if target_was_delivery
            && replacement_terminal.contract.output_kind != WorkflowOutputKind::Synthesis
        {
            return Err("replacement for the delivery step must end in synthesis".to_string());
        }
        if !target_was_delivery
            && replacement_steps
                .iter()
                .any(|step| step.contract.output_kind == WorkflowOutputKind::Synthesis)
        {
            return Err("non-terminal workflow revision cannot introduce synthesis".to_string());
        }

        let mut revised_steps = self.plan.steps[..target_index].to_vec();
        revised_steps.extend(replacement_steps.iter().cloned());
        revised_steps.extend(self.plan.steps[target_index + 1..].iter().cloned());
        let replacement_terminal_id = replacement_terminal.id.clone();
        for step in revised_steps
            .iter_mut()
            .skip(target_index + replacement_count)
        {
            for dependency in &mut step.access {
                if dependency == &revision.target_step_id {
                    *dependency = replacement_terminal_id.clone();
                }
            }
            step.contract.input_steps = step.access.clone();
        }

        let mut revised_plan = self.plan.clone();
        revised_plan.steps = revised_steps;
        revised_plan.validate(allowed_models)?;

        self.steps.remove(&revision.target_step_id);
        for step in &replacement_steps {
            self.steps.insert(
                step.id.clone(),
                WorkflowStepCheckpoint {
                    step_id: step.id.clone(),
                    status: WorkflowStepStatus::Pending,
                    attempts: 0,
                    model: step.model.clone(),
                    output: None,
                    evidence_json: String::new(),
                    evidence_count: 0,
                    latency_ms: 0,
                    total_tokens: 0,
                    credit: None,
                    semantic: WorkflowStepSemanticState {
                        output_kind: step.contract.output_kind.clone(),
                        ..WorkflowStepSemanticState::default()
                    },
                    error: None,
                    verification_repair_rounds: 0,
                    updated_at_ms: now_ms,
                },
            );
        }
        self.plan = revised_plan;
        self.finalized = false;
        self.updated_at_ms = now_ms;
        let record = WorkflowPlanRevisionRecord {
            revision: self.plan_revisions.len() + 1,
            target_step_id: revision.target_step_id,
            replacement_step_ids: replacement_ids,
            rationale: revision.rationale,
            applied_at_ms: now_ms,
        };
        self.plan_revisions.push(record.clone());
        self.validate(allowed_models)?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WorkflowBudget, WorkflowPlanIr, WORKFLOW_IR_SCHEMA};

    fn step(
        id: &str,
        model: &str,
        access: &[&str],
        output_kind: WorkflowOutputKind,
    ) -> WorkflowPlanStep {
        WorkflowPlanStep {
            id: id.to_string(),
            role: id.to_string(),
            model: model.to_string(),
            subtask: id.to_string(),
            access: access.iter().map(|value| (*value).to_string()).collect(),
            tool_policy: WorkflowToolPolicy::None,
            contract: WorkflowStepContract {
                input_steps: access.iter().map(|value| (*value).to_string()).collect(),
                output_kind,
                completion: WorkflowCompletionCriteria::default(),
            },
        }
    }

    fn checkpoint() -> WorkflowExecutionCheckpoint {
        WorkflowExecutionCheckpoint::new(
            "revision-test",
            WorkflowPlanIr {
                schema: WORKFLOW_IR_SCHEMA.to_string(),
                workflow_id: "workflow".to_string(),
                objective: "objective".to_string(),
                effort: "pro".to_string(),
                policy: "auto_router".to_string(),
                coordinator_model: "model-a".to_string(),
                prompt_profile: "baseline".to_string(),
                parallel_read_only_specialists: false,
                steps: vec![
                    step("inspect", "model-a", &[], WorkflowOutputKind::Evidence),
                    step(
                        "deliver",
                        "model-b",
                        &["inspect"],
                        WorkflowOutputKind::Synthesis,
                    ),
                ],
                budget: WorkflowBudget {
                    max_steps: 3,
                    max_models: 2,
                    max_model_turns_per_step: 2,
                    max_tool_calls_per_step: 2,
                    max_output_tokens_per_step: 2_000,
                },
            },
            1,
        )
    }

    #[test]
    fn revision_splits_failed_step_and_rewires_descendant() {
        let mut checkpoint = checkpoint();
        checkpoint.fail_step("inspect", "failed", 2).unwrap();
        let record = checkpoint
            .apply_plan_revision(
                WorkflowPlanRevision {
                    schema: WORKFLOW_REVISION_SCHEMA.to_string(),
                    target_step_id: "inspect".to_string(),
                    rationale: "separate diagnosis from evidence collection".to_string(),
                    replacement_steps: vec![
                        WorkflowReplacementStep {
                            id: "diagnose".to_string(),
                            role: "diagnostician".to_string(),
                            model: "model-a".to_string(),
                            subtask: "diagnose the failure".to_string(),
                            access: Vec::new(),
                            tool_policy: WorkflowToolPolicy::None,
                            output_kind: WorkflowOutputKind::Analysis,
                        },
                        WorkflowReplacementStep {
                            id: "inspect-retry".to_string(),
                            role: "evidence_researcher".to_string(),
                            model: "model-b".to_string(),
                            subtask: "collect evidence using the diagnosis".to_string(),
                            access: vec!["diagnose".to_string()],
                            tool_policy: WorkflowToolPolicy::ReadOnlyEvidence,
                            output_kind: WorkflowOutputKind::Evidence,
                        },
                    ],
                },
                &["model-a".to_string(), "model-b".to_string()],
                3,
            )
            .unwrap();
        assert_eq!(record.replacement_step_ids, ["diagnose", "inspect-retry"]);
        assert_eq!(checkpoint.plan.steps[2].access, ["inspect-retry"]);
        assert!(!checkpoint.steps.contains_key("inspect"));
        assert_eq!(checkpoint.plan_revisions.len(), 1);
    }

    #[test]
    fn revision_rejects_resolved_target_and_budget_overflow() {
        let mut checkpoint = checkpoint();
        checkpoint
            .complete_step(
                "inspect",
                "model-a",
                "evidence".to_string(),
                "[]".to_string(),
                2,
            )
            .unwrap();
        let error = checkpoint
            .apply_plan_revision(
                WorkflowPlanRevision {
                    schema: WORKFLOW_REVISION_SCHEMA.to_string(),
                    target_step_id: "inspect".to_string(),
                    rationale: "replace completed work".to_string(),
                    replacement_steps: vec![WorkflowReplacementStep {
                        id: "replacement".to_string(),
                        role: "worker".to_string(),
                        model: "model-a".to_string(),
                        subtask: "replace".to_string(),
                        access: Vec::new(),
                        tool_policy: WorkflowToolPolicy::None,
                        output_kind: WorkflowOutputKind::Evidence,
                    }],
                },
                &["model-a".to_string(), "model-b".to_string()],
                3,
            )
            .unwrap_err();
        assert!(error.contains("resolved step"));
    }

    #[test]
    fn revision_harness_rejects_a_renamed_copy_of_the_failed_strategy() {
        let checkpoint = checkpoint();
        let harness = WorkflowRevisionHarness::new(WorkflowRevisionRequest {
            plan: checkpoint.plan.clone(),
            failed_step_id: "inspect".to_string(),
            failure: "provider rejected the request".to_string(),
            preserved_outputs: Vec::new(),
            allowed_models: vec!["model-a".to_string(), "model-b".to_string()],
        });

        let error = harness
            .parse_revision(
                r#"{
                    "schema":"cindx.workflow.revision.v1",
                    "target_step_id":"inspect",
                    "rationale":"try the same thing again",
                    "replacement_steps":[{
                        "id":"inspect-again",
                        "role":"inspect",
                        "model":"model-a",
                        "subtask":"inspect",
                        "access":[],
                        "tool_policy":"none",
                        "output_kind":"evidence"
                    }]
                }"#,
            )
            .unwrap_err();

        assert!(error.contains("repeats the failed strategy"));
    }
}
