use std::{collections::BTreeMap, sync::Arc};

use agent_core::ModelRole;
use agent_runtime::{AgentRunControl, RunStageClass};
use model_provider::MODEL_REQUEST_CANCELLED;
use orchestrator::{
    candidate_pair_review_prompt, compare_team_and_anchor_order_invariant,
    direct_anchor_response_prompt, parse_candidate_pair_review, ConductorExecutionContract,
    TeamAnchorComparison, WorkflowOutputKind, WorkflowPlanIr, WorkflowToolPolicy,
};

use crate::{
    collaboration_models::{
        PromptDependencyOutput, PromptEvaluationRunner, PromptEvaluationWorkerRequest,
        PromptExecutionStep,
    },
    collaboration_service::CollaborationCompletion,
    parallel_execution::{model_job_supervisor, ParallelJobSupervisor},
};

#[derive(Debug)]
pub(super) struct PromptDirectAnchorResult {
    pub(super) output: String,
    pub(super) latency_ms: u64,
    pub(super) total_tokens: u64,
}

#[derive(Debug)]
pub(super) struct PromptTeamAnchorComparisonResult {
    pub(super) comparison: TeamAnchorComparison,
    pub(super) total_tokens: u64,
}

pub(super) fn start_prompt_direct_anchor(
    objective: &str,
    plan: &WorkflowPlanIr,
    runner: PromptEvaluationRunner,
) -> Result<Option<ParallelJobSupervisor<CollaborationCompletion>>, String> {
    let model = plan
        .steps
        .iter()
        .find(|step| matches!(step.role.as_str(), "worker" | "thinker" | "executor"))
        .or_else(|| plan.steps.first())
        .map(|step| step.model.clone())
        .unwrap_or_else(|| plan.coordinator_model.clone());
    if model.trim().is_empty() {
        return Ok(None);
    }
    let request = PromptEvaluationWorkerRequest {
        stage: "prompt_evaluation_direct_anchor".to_string(),
        stage_class: RunStageClass::Candidate,
        role: ModelRole::Executor,
        model,
        prompt: direct_anchor_response_prompt(objective, ""),
        tool_policy: WorkflowToolPolicy::None,
        max_model_turns: 1,
        max_tool_calls: 0,
        max_output_tokens: 2_048,
    };
    let mut supervisor = model_job_supervisor::<CollaborationCompletion>();
    supervisor
        .submit(
            0,
            "prompt-direct-anchor",
            Box::new(move |branch_cancellation| runner(request, branch_cancellation)),
        )
        .map_err(|error| format!("failed to start prompt direct anchor: {error}"))?;
    Ok(Some(supervisor))
}

pub(super) fn finish_prompt_direct_anchor(
    supervisor: &mut Option<ParallelJobSupervisor<CollaborationCompletion>>,
    control: Option<&Arc<AgentRunControl>>,
) -> Option<PromptDirectAnchorResult> {
    let supervisor = supervisor.as_mut()?;
    if control.is_some_and(|control| control.should_stop()) {
        supervisor.cancel_all();
    }
    let completion = supervisor.recv()?.result.ok()?;
    let output = completion
        .content
        .or(completion.partial_content)
        .filter(|output| !output.trim().is_empty())?;
    let total_tokens = completion
        .usage
        .get("total_tokens")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    Some(PromptDirectAnchorResult {
        output,
        latency_ms: completion.latency_ms,
        total_tokens,
    })
}

pub(super) fn compare_prompt_team_with_anchor(
    objective: &str,
    team_output: &str,
    anchor_output: &str,
    plan: &WorkflowPlanIr,
    runner: PromptEvaluationRunner,
    control: Option<&Arc<AgentRunControl>>,
) -> Result<Option<PromptTeamAnchorComparisonResult>, String> {
    if team_output.trim().is_empty() || anchor_output.trim().is_empty() {
        return Ok(None);
    }
    let reviewer_model = plan
        .steps
        .iter()
        .find(|step| matches!(step.role.as_str(), "reviewer" | "verifier"))
        .map(|step| step.model.clone())
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| plan.coordinator_model.clone());
    if reviewer_model.trim().is_empty() {
        return Err("team-anchor comparison has no reviewer model".to_string());
    }
    let requests = [
        PromptEvaluationWorkerRequest {
            stage: "prompt_evaluation_team_anchor_forward".to_string(),
            stage_class: RunStageClass::Reviewer,
            role: ModelRole::Reviewer,
            model: reviewer_model.clone(),
            prompt: candidate_pair_review_prompt(objective, team_output, anchor_output),
            tool_policy: WorkflowToolPolicy::None,
            max_model_turns: 1,
            max_tool_calls: 0,
            max_output_tokens: 1_024,
        },
        PromptEvaluationWorkerRequest {
            stage: "prompt_evaluation_team_anchor_reverse".to_string(),
            stage_class: RunStageClass::Reviewer,
            role: ModelRole::Reviewer,
            model: reviewer_model,
            prompt: candidate_pair_review_prompt(objective, anchor_output, team_output),
            tool_policy: WorkflowToolPolicy::None,
            max_model_turns: 1,
            max_tool_calls: 0,
            max_output_tokens: 1_024,
        },
    ];
    let mut supervisor = model_job_supervisor::<CollaborationCompletion>();
    for (job_id, request) in requests.into_iter().enumerate() {
        let runner = Arc::clone(&runner);
        supervisor
            .submit(
                job_id,
                "prompt-team-anchor-review",
                Box::new(move |branch_cancellation| runner(request, branch_cancellation)),
            )
            .map_err(|error| format!("failed to start team-anchor reviewer: {error}"))?;
    }

    let mut responses = [None::<String>, None::<String>];
    let mut total_tokens = 0u64;
    while supervisor.pending() > 0 {
        if control.is_some_and(|control| control.should_stop()) {
            supervisor.cancel_all();
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
        let completion = supervisor
            .recv()
            .ok_or_else(|| "team-anchor reviewer ended without a result".to_string())?;
        let job_id = completion.job_id;
        let completion = completion
            .result
            .map_err(|error| format!("team-anchor reviewer {job_id} failed: {error}"))?;
        total_tokens = total_tokens.saturating_add(
            completion
                .usage
                .get("total_tokens")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default(),
        );
        responses[job_id] = completion
            .content
            .or(completion.partial_content)
            .filter(|output| !output.trim().is_empty());
    }
    let forward = parse_candidate_pair_review(
        responses[0]
            .as_deref()
            .ok_or_else(|| "forward team-anchor reviewer returned no content".to_string())?,
    )?;
    let reverse = parse_candidate_pair_review(
        responses[1]
            .as_deref()
            .ok_or_else(|| "reverse team-anchor reviewer returned no content".to_string())?,
    )?;
    Ok(Some(PromptTeamAnchorComparisonResult {
        comparison: compare_team_and_anchor_order_invariant(forward, reverse)?,
        total_tokens,
    }))
}

pub(super) fn prompt_execution_quality_gate(
    plan: &WorkflowPlanIr,
    steps: &[PromptExecutionStep],
    contract: &ConductorExecutionContract,
) -> bool {
    let Some(final_step) = plan.steps.last() else {
        return false;
    };
    let final_step_completed = steps
        .iter()
        .find(|step| step.id == final_step.id)
        .is_some_and(PromptExecutionStep::succeeded);
    let root_steps = plan
        .steps
        .iter()
        .filter(|step| step.access.is_empty())
        .collect::<Vec<_>>();
    let successful_roots = root_steps
        .iter()
        .filter(|planned| {
            steps
                .iter()
                .find(|executed| executed.id == planned.id)
                .is_some_and(PromptExecutionStep::succeeded)
        })
        .count();
    let root_gate_met = root_steps.is_empty()
        || successful_roots >= contract.required_successes_for_layer(root_steps.len());
    let verification_gate_met = !contract.verification_required
        || steps.iter().any(|step| {
            step.succeeded() && matches!(step.role.as_str(), "verifier" | "synthesizer")
        });
    final_step_completed && root_gate_met && verification_gate_met
}

pub(super) fn select_final_output(
    plan: &WorkflowPlanIr,
    steps: &[PromptExecutionStep],
    outputs: &BTreeMap<String, PromptDependencyOutput>,
) -> String {
    if let Some(output) = plan
        .steps
        .last()
        .and_then(|step| outputs.get(&step.id))
        .filter(|output| !output.content.trim().is_empty())
    {
        return output.content.clone();
    }

    steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.usable())
        .max_by_key(|(execution_index, step)| {
            let output_rank = plan
                .steps
                .iter()
                .find(|planned| planned.id == step.id)
                .map(|planned| output_kind_rank(&planned.contract.output_kind))
                .unwrap_or_default();
            let completion_rank = usize::from(step.succeeded());
            (
                output_rank,
                completion_rank,
                step.evidence_count,
                *execution_index,
            )
        })
        .map(|(_, step)| step.output.clone())
        .unwrap_or_default()
}

fn output_kind_rank(kind: &WorkflowOutputKind) -> usize {
    match kind {
        WorkflowOutputKind::Analysis => 0,
        WorkflowOutputKind::Evidence => 1,
        WorkflowOutputKind::Verification => 2,
        WorkflowOutputKind::Synthesis => 3,
    }
}
