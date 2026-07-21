use super::*;

pub(crate) fn merge_unique_feedback_entries(target: &mut Vec<String>, entries: Vec<String>) {
    for entry in entries {
        if !entry.trim().is_empty() && !target.iter().any(|existing| existing == &entry) {
            target.push(entry);
        }
    }
}

pub(crate) fn merge_actionable_feedback(
    mut primary: ActionableSideInformation,
    secondary: ActionableSideInformation,
) -> ActionableSideInformation {
    if primary.summary.trim().is_empty() {
        primary.summary = secondary.summary.clone();
    } else if !secondary.summary.trim().is_empty() && primary.summary != secondary.summary {
        primary.summary.push('\n');
        primary.summary.push_str(&secondary.summary);
    }
    merge_unique_feedback_entries(
        &mut primary.passed_constraints,
        secondary.passed_constraints,
    );
    merge_unique_feedback_entries(
        &mut primary.failed_constraints,
        secondary.failed_constraints,
    );
    merge_unique_feedback_entries(&mut primary.errors, secondary.errors);
    merge_unique_feedback_entries(&mut primary.suggested_changes, secondary.suggested_changes);
    primary
}

pub(crate) fn average_step_score_maps(
    mut primary: BTreeMap<String, f64>,
    secondary: BTreeMap<String, f64>,
) -> BTreeMap<String, f64> {
    for (step_id, score) in secondary {
        primary
            .entry(step_id)
            .and_modify(|current| *current = (*current + score) / 2.0)
            .or_insert(score);
    }
    primary
}

pub(crate) fn reverse_prompt_pairwise_payload(
    payload: PromptPairwiseEvaluationPayload,
) -> PromptPairwiseEvaluationPayload {
    PromptPairwiseEvaluationPayload {
        score_a: payload.score_b,
        score_b: payload.score_a,
        safety_violations_a: payload.safety_violations_b,
        safety_violations_b: payload.safety_violations_a,
        step_scores_a: payload.step_scores_b,
        step_scores_b: payload.step_scores_a,
        feedback_a: payload.feedback_b,
        feedback_b: payload.feedback_a,
    }
}

pub(crate) fn aggregate_prompt_pairwise_payloads(
    first: PromptPairwiseEvaluationPayload,
    second: PromptPairwiseEvaluationPayload,
) -> PromptPairwiseEvaluationPayload {
    PromptPairwiseEvaluationPayload {
        score_a: (first.score_a + second.score_a) / 2.0,
        score_b: (first.score_b + second.score_b) / 2.0,
        safety_violations_a: first.safety_violations_a.max(second.safety_violations_a),
        safety_violations_b: first.safety_violations_b.max(second.safety_violations_b),
        step_scores_a: average_step_score_maps(first.step_scores_a, second.step_scores_a),
        step_scores_b: average_step_score_maps(first.step_scores_b, second.step_scores_b),
        feedback_a: merge_actionable_feedback(first.feedback_a, second.feedback_a),
        feedback_b: merge_actionable_feedback(first.feedback_b, second.feedback_b),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PromptPlanCandidate {
    pub(crate) genome: ConductorPromptGenome,
    pub(crate) plan: Option<WorkflowPlanIr>,
    pub(crate) raw_output: String,
    pub(crate) latency_ms: u64,
    pub(crate) total_tokens: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct PromptExecutionStep {
    pub(crate) id: String,
    pub(crate) role: String,
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) attempts: usize,
    pub(crate) succeeded: bool,
    pub(crate) output: String,
    pub(crate) tool_calls: Vec<AgentEvaluationToolTrace>,
    pub(crate) errors: Vec<String>,
    pub(crate) latency_ms: u64,
    pub(crate) total_tokens: u64,
    pub(crate) evidence_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct PromptWorkflowExecution {
    pub(crate) succeeded: bool,
    pub(crate) final_output: String,
    pub(crate) steps: Vec<PromptExecutionStep>,
    pub(crate) latency_ms: u64,
    pub(crate) total_tokens: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct PromptExecutionCandidate {
    pub(crate) plan: PromptPlanCandidate,
    pub(crate) execution: PromptWorkflowExecution,
}

#[derive(Debug, Clone)]
pub(crate) struct PromptEvaluationWorkerRequest {
    pub(crate) role: ModelRole,
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) tool_policy: WorkflowToolPolicy,
    pub(crate) max_model_turns: usize,
    pub(crate) max_tool_calls: usize,
}

pub(crate) type PromptEvaluationRunner =
    Arc<dyn Fn(PromptEvaluationWorkerRequest) -> CollaborationCompletion + Send + Sync>;

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct PromptReplayCase {
    pub(crate) objective: String,
    pub(crate) task_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOfflineCase {
    pub(crate) id: String,
    pub(crate) objective: String,
    pub(crate) task_class: String,
    pub(crate) project_id: String,
    pub(crate) source_run_id: String,
    pub(crate) split: PromptEvaluationSplit,
}
