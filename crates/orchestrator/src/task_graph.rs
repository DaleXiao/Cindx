use crate::{
    WorkflowExecutionCheckpoint, WorkflowOutputKind, WorkflowPlanStep, WorkflowStepStatus,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowStepDisposition {
    Resolved,
    ResumeRunning,
    StartAttempt,
    Exhausted,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkflowExecutionFrontier {
    pub ready_steps: Vec<String>,
    pub resumable_steps: Vec<String>,
    pub resolved_steps: Vec<String>,
    pub exhausted_steps: Vec<String>,
    pub blocked_steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkflowDeliveryFrontier {
    pub target_step_id: Option<String>,
    pub required_steps: Vec<String>,
    pub remaining_steps: Vec<String>,
    pub runnable_steps: Vec<String>,
    pub remaining_layers: usize,
}

impl WorkflowExecutionFrontier {
    pub fn runnable_steps(&self) -> Vec<String> {
        self.resumable_steps
            .iter()
            .chain(self.ready_steps.iter())
            .cloned()
            .collect()
    }

    pub fn is_complete(&self, total_steps: usize) -> bool {
        self.resolved_steps.len() == total_steps
    }

    pub fn is_stalled(&self, total_steps: usize) -> bool {
        !self.is_complete(total_steps)
            && self.ready_steps.is_empty()
            && self.resumable_steps.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowStepClaim {
    pub step_id: String,
    pub model: String,
    pub resumed: bool,
}

impl WorkflowExecutionCheckpoint {
    pub fn execution_frontier(
        &self,
        attempt_limit: usize,
    ) -> Result<WorkflowExecutionFrontier, String> {
        self.execution_frontier_with_recovery(attempt_limit, false)
    }

    pub fn execution_frontier_with_partial_recovery(
        &self,
        attempt_limit: usize,
    ) -> Result<WorkflowExecutionFrontier, String> {
        self.execution_frontier_with_recovery(attempt_limit, true)
    }

    pub fn delivery_frontier(
        &self,
        attempt_limit: usize,
    ) -> Result<WorkflowDeliveryFrontier, String> {
        self.delivery_frontier_with_recovery(attempt_limit, false)
    }

    pub fn delivery_frontier_with_partial_recovery(
        &self,
        attempt_limit: usize,
    ) -> Result<WorkflowDeliveryFrontier, String> {
        self.delivery_frontier_with_recovery(attempt_limit, true)
    }

    fn delivery_frontier_with_recovery(
        &self,
        attempt_limit: usize,
        allow_partial_recovery: bool,
    ) -> Result<WorkflowDeliveryFrontier, String> {
        let Some(target) = delivery_target(&self.plan.steps) else {
            return Ok(WorkflowDeliveryFrontier::default());
        };
        let by_id = self
            .plan
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step))
            .collect::<BTreeMap<_, _>>();
        let mut required = BTreeSet::new();
        collect_delivery_requirements(&target.id, &by_id, &mut required)?;
        let execution =
            self.execution_frontier_with_recovery(attempt_limit, allow_partial_recovery)?;
        let runnable = execution
            .runnable_steps()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let resolved = execution
            .resolved_steps
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut depth_cache = BTreeMap::new();
        let remaining_layers = unresolved_delivery_depth(
            &target.id,
            &by_id,
            &resolved,
            &mut depth_cache,
            &mut BTreeSet::new(),
        )?;
        Ok(WorkflowDeliveryFrontier {
            target_step_id: Some(target.id.clone()),
            required_steps: required.iter().cloned().collect(),
            remaining_steps: required
                .iter()
                .filter(|step_id| !resolved.contains(*step_id))
                .cloned()
                .collect(),
            runnable_steps: required
                .iter()
                .filter(|step_id| runnable.contains(*step_id))
                .cloned()
                .collect(),
            remaining_layers,
        })
    }

    fn execution_frontier_with_recovery(
        &self,
        attempt_limit: usize,
        allow_partial_recovery: bool,
    ) -> Result<WorkflowExecutionFrontier, String> {
        let mut frontier = WorkflowExecutionFrontier::default();
        for step in &self.plan.steps {
            match self.step_disposition_with_recovery(
                &step.id,
                attempt_limit,
                allow_partial_recovery,
            )? {
                WorkflowStepDisposition::Resolved => frontier.resolved_steps.push(step.id.clone()),
                WorkflowStepDisposition::ResumeRunning => {
                    frontier.resumable_steps.push(step.id.clone())
                }
                WorkflowStepDisposition::StartAttempt => frontier.ready_steps.push(step.id.clone()),
                WorkflowStepDisposition::Exhausted => {
                    frontier.exhausted_steps.push(step.id.clone())
                }
                WorkflowStepDisposition::Blocked => frontier.blocked_steps.push(step.id.clone()),
            }
        }
        Ok(frontier)
    }

    pub fn claim_steps(
        &mut self,
        step_ids: &[String],
        attempt_limit: usize,
        now_ms: u64,
    ) -> Result<Vec<WorkflowStepClaim>, String> {
        self.claim_steps_with_recovery(step_ids, attempt_limit, now_ms, false)
    }

    pub fn claim_steps_with_partial_recovery(
        &mut self,
        step_ids: &[String],
        attempt_limit: usize,
        now_ms: u64,
    ) -> Result<Vec<WorkflowStepClaim>, String> {
        self.claim_steps_with_recovery(step_ids, attempt_limit, now_ms, true)
    }

    fn claim_steps_with_recovery(
        &mut self,
        step_ids: &[String],
        attempt_limit: usize,
        now_ms: u64,
        allow_partial_recovery: bool,
    ) -> Result<Vec<WorkflowStepClaim>, String> {
        let frontier =
            self.execution_frontier_with_recovery(attempt_limit, allow_partial_recovery)?;
        let runnable = frontier
            .runnable_steps()
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        let requested = step_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if requested.len() != step_ids.len() {
            return Err("workflow step claim contains duplicate ids".to_string());
        }
        if let Some(step_id) = requested
            .iter()
            .find(|step_id| !runnable.contains(*step_id))
        {
            return Err(format!(
                "workflow step {step_id} is not runnable in the current frontier"
            ));
        }

        let mut claims = Vec::with_capacity(step_ids.len());
        for step_id in step_ids {
            let resumed = frontier.resumable_steps.iter().any(|id| id == step_id);
            let model = self
                .steps
                .get(step_id)
                .map(|step| step.model.clone())
                .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
            if !resumed {
                self.begin_step_with_attempt_limit(step_id, &model, attempt_limit, now_ms)?;
            }
            claims.push(WorkflowStepClaim {
                step_id: step_id.clone(),
                model,
                resumed,
            });
        }
        Ok(claims)
    }

    pub fn step_disposition(
        &self,
        step_id: &str,
        attempt_limit: usize,
    ) -> Result<WorkflowStepDisposition, String> {
        self.step_disposition_with_recovery(step_id, attempt_limit, false)
    }

    fn step_disposition_with_recovery(
        &self,
        step_id: &str,
        attempt_limit: usize,
        allow_partial_recovery: bool,
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
        let unresolved = plan_step
            .access
            .iter()
            .filter(|dependency| {
                self.steps.get(*dependency).is_none_or(|step| {
                    !matches!(
                        step.status,
                        WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
                    )
                })
            })
            .collect::<Vec<_>>();
        if !unresolved.is_empty() {
            let resolved_count = plan_step.access.len().saturating_sub(unresolved.len());
            let recoverable_output = matches!(
                plan_step.contract.output_kind,
                WorkflowOutputKind::Verification | WorkflowOutputKind::Synthesis
            );
            let unavailable_dependencies = unresolved.iter().all(|dependency| {
                self.steps.get(*dependency).is_some_and(|step| {
                    step.status == WorkflowStepStatus::Failed
                        && step.attempts >= attempt_limit.max(1)
                })
            });
            if !allow_partial_recovery
                || !recoverable_output
                || resolved_count == 0
                || !unavailable_dependencies
            {
                return Ok(WorkflowStepDisposition::Blocked);
            }
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

fn delivery_target(steps: &[WorkflowPlanStep]) -> Option<&WorkflowPlanStep> {
    steps
        .iter()
        .rev()
        .find(|step| step.contract.output_kind == WorkflowOutputKind::Synthesis)
        .or_else(|| {
            steps
                .iter()
                .rev()
                .find(|step| step.contract.output_kind == WorkflowOutputKind::Verification)
        })
        .or_else(|| steps.last())
}

fn step_inputs(step: &WorkflowPlanStep) -> BTreeSet<&str> {
    step.access
        .iter()
        .chain(step.contract.input_steps.iter())
        .map(String::as_str)
        .collect()
}

fn collect_delivery_requirements(
    step_id: &str,
    by_id: &BTreeMap<&str, &WorkflowPlanStep>,
    required: &mut BTreeSet<String>,
) -> Result<(), String> {
    if !required.insert(step_id.to_string()) {
        return Ok(());
    }
    let step = by_id
        .get(step_id)
        .ok_or_else(|| format!("workflow delivery target references unknown step: {step_id}"))?;
    for dependency in step_inputs(step) {
        collect_delivery_requirements(dependency, by_id, required)?;
    }
    Ok(())
}

fn unresolved_delivery_depth(
    step_id: &str,
    by_id: &BTreeMap<&str, &WorkflowPlanStep>,
    resolved: &BTreeSet<String>,
    cache: &mut BTreeMap<String, usize>,
    visiting: &mut BTreeSet<String>,
) -> Result<usize, String> {
    if resolved.contains(step_id) {
        return Ok(0);
    }
    if let Some(depth) = cache.get(step_id) {
        return Ok(*depth);
    }
    if !visiting.insert(step_id.to_string()) {
        return Err(format!(
            "workflow delivery path contains a cycle at {step_id}"
        ));
    }
    let step = by_id
        .get(step_id)
        .ok_or_else(|| format!("workflow delivery path references unknown step: {step_id}"))?;
    let dependency_depth = step_inputs(step)
        .into_iter()
        .map(|dependency| unresolved_delivery_depth(dependency, by_id, resolved, cache, visiting))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .unwrap_or_default();
    visiting.remove(step_id);
    let depth = dependency_depth.saturating_add(1);
    cache.insert(step_id.to_string(), depth);
    Ok(depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        WorkflowBudget, WorkflowPlanIr, WorkflowPlanStep, WorkflowStepContract, WorkflowToolPolicy,
        WORKFLOW_IR_SCHEMA,
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
    fn frontier_and_claims_follow_dependencies_without_restarting_running_work() {
        let mut checkpoint = checkpoint();
        let frontier = checkpoint.execution_frontier(2).unwrap();
        assert_eq!(frontier.ready_steps, vec!["inspect"]);
        assert_eq!(frontier.blocked_steps, vec!["synthesize"]);
        assert!(!frontier.is_stalled(2));

        let claims = checkpoint
            .claim_steps(&["inspect".to_string()], 2, 10)
            .unwrap();
        assert_eq!(claims.len(), 1);
        assert!(!claims[0].resumed);
        assert_eq!(checkpoint.steps["inspect"].attempts, 1);

        let resumed = checkpoint.execution_frontier(2).unwrap();
        assert_eq!(resumed.resumable_steps, vec!["inspect"]);
        let claims = checkpoint
            .claim_steps(&["inspect".to_string()], 2, 11)
            .unwrap();
        assert!(claims[0].resumed);
        assert_eq!(checkpoint.steps["inspect"].attempts, 1);

        checkpoint
            .complete_step(
                "inspect",
                "worker",
                "evidence".to_string(),
                "[]".to_string(),
                12,
            )
            .unwrap();
        let next = checkpoint.execution_frontier(2).unwrap();
        assert_eq!(next.resolved_steps, vec!["inspect"]);
        assert_eq!(next.ready_steps, vec!["synthesize"]);
    }

    #[test]
    fn delivery_frontier_excludes_speculation_and_tracks_remaining_layers() {
        let base = checkpoint();
        let mut plan = base.plan;
        plan.steps.insert(
            1,
            WorkflowPlanStep {
                id: "speculative-note".to_string(),
                role: "worker".to_string(),
                model: "worker".to_string(),
                subtask: "optional note".to_string(),
                access: Vec::new(),
                tool_policy: WorkflowToolPolicy::None,
                contract: WorkflowStepContract::inferred("worker", &[], &WorkflowToolPolicy::None),
            },
        );
        plan.budget.max_steps = 3;
        let mut checkpoint = WorkflowExecutionCheckpoint::new("delivery", plan, 1);

        let initial = checkpoint.delivery_frontier(1).unwrap();
        assert_eq!(initial.target_step_id.as_deref(), Some("synthesize"));
        assert_eq!(initial.required_steps, vec!["inspect", "synthesize"]);
        assert_eq!(initial.runnable_steps, vec!["inspect"]);
        assert_eq!(initial.remaining_layers, 2);

        checkpoint
            .complete_step(
                "inspect",
                "worker",
                "evidence".to_string(),
                "[]".to_string(),
                2,
            )
            .unwrap();
        let terminal = checkpoint.delivery_frontier(1).unwrap();
        assert_eq!(terminal.remaining_steps, vec!["synthesize"]);
        assert_eq!(terminal.runnable_steps, vec!["synthesize"]);
        assert_eq!(terminal.remaining_layers, 1);
    }

    #[test]
    fn stalled_frontier_exposes_exhausted_roots_and_blocked_dependents() {
        let mut checkpoint = checkpoint();
        checkpoint
            .prepare_step_attempt("inspect", "worker", 1, 1)
            .unwrap();
        checkpoint.fail_step("inspect", "failed", 2).unwrap();

        let frontier = checkpoint.execution_frontier(1).unwrap();
        assert_eq!(frontier.exhausted_steps, vec!["inspect"]);
        assert_eq!(frontier.blocked_steps, vec!["synthesize"]);
        assert!(frontier.is_stalled(2));
    }

    #[test]
    fn partial_recovery_only_unlocks_terminal_synthesis_dependencies() {
        let base = checkpoint();
        let mut plan = base.plan;
        let second_root = WorkflowPlanStep {
            id: "inspect-backup".to_string(),
            role: "worker".to_string(),
            model: "worker".to_string(),
            subtask: "inspect backup".to_string(),
            access: Vec::new(),
            tool_policy: WorkflowToolPolicy::ReadOnlyEvidence,
            contract: WorkflowStepContract::inferred(
                "worker",
                &[],
                &WorkflowToolPolicy::ReadOnlyEvidence,
            ),
        };
        plan.steps.insert(1, second_root);
        let synthesis = plan.steps.last_mut().unwrap();
        synthesis.access = vec!["inspect".to_string(), "inspect-backup".to_string()];
        synthesis.contract = WorkflowStepContract::inferred(
            "synthesizer",
            &synthesis.access,
            &WorkflowToolPolicy::None,
        );
        plan.budget.max_steps = 3;
        let mut checkpoint = WorkflowExecutionCheckpoint::new("partial", plan, 1);

        checkpoint
            .complete_step(
                "inspect",
                "worker",
                "grounded result".to_string(),
                "[]".to_string(),
                2,
            )
            .unwrap();
        checkpoint
            .prepare_step_attempt("inspect-backup", "worker", 1, 3)
            .unwrap();
        checkpoint
            .fail_step("inspect-backup", "provider failure", 4)
            .unwrap();

        let strict = checkpoint.execution_frontier(1).unwrap();
        assert_eq!(strict.blocked_steps, vec!["synthesize"]);
        let recoverable = checkpoint
            .execution_frontier_with_partial_recovery(1)
            .unwrap();
        assert_eq!(recoverable.ready_steps, vec!["synthesize"]);
        let claims = checkpoint
            .claim_steps_with_partial_recovery(&["synthesize".to_string()], 1, 5)
            .unwrap();
        assert_eq!(claims[0].step_id, "synthesize");
        assert!(!claims[0].resumed);
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
