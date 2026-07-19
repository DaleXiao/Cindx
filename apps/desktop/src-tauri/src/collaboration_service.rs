use agent_core::{Message, MessageRole, Metadata};
use agent_runtime::AgentLoopState;
use orchestrator::{
    ConductorPromptGenome, WorkflowExecutionCheckpoint, WorkflowPlanIr, WorkflowToolPolicy,
};
use serde::{Deserialize, Serialize};

const COLLABORATION_WORKER_FINALIZATION_TURNS: usize = 1;

pub(crate) const WORKFLOW_RESUMABLE_ERROR_PREFIX: &str = "workflow checkpoint saved:";

#[derive(Debug)]
pub(crate) struct AdaptiveCollaborationSpec {
    pub(crate) step_index: usize,
    pub(crate) step_id: String,
    pub(crate) role: String,
    pub(crate) stage: String,
    pub(crate) model: String,
    pub(crate) subtask: String,
    pub(crate) prompt: String,
    pub(crate) request_id: String,
    pub(crate) access: Vec<String>,
    pub(crate) tool_policy: WorkflowToolPolicy,
    pub(crate) max_attempts: usize,
    pub(crate) max_model_turns: usize,
    pub(crate) max_tool_calls: usize,
}

#[derive(Debug)]
pub(crate) struct CollaborationCompletion {
    pub(crate) content: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) latency_ms: u64,
    pub(crate) usage: Metadata,
    pub(crate) evidence: Vec<CollaborationEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CollaborationEvidence {
    pub(crate) source_step: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    #[serde(default)]
    pub(crate) request: String,
    pub(crate) status: String,
    pub(crate) output: String,
}

impl CollaborationCompletion {
    pub(crate) fn failed(error: impl Into<String>) -> Self {
        Self {
            content: None,
            error: Some(error.into()),
            latency_ms: 0,
            usage: Metadata::new(),
            evidence: Vec::new(),
        }
    }
}

pub(crate) fn effective_workflow_model_turn_budget(
    plan: &WorkflowPlanIr,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> usize {
    plan.budget
        .max_model_turns_per_step
        .saturating_add(checkpoint.additional_model_turns_per_step)
        .max(1)
}

pub(crate) fn effective_workflow_step_attempt_budget(
    genome: &ConductorPromptGenome,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> usize {
    genome
        .max_step_attempts
        .max(1)
        .saturating_mul(checkpoint.continuations.saturating_add(1))
}

pub(crate) fn collaboration_worker_runtime_turn_limit(
    max_model_turns: usize,
    has_tools: bool,
) -> usize {
    max_model_turns.max(1).saturating_add(if has_tools {
        COLLABORATION_WORKER_FINALIZATION_TURNS
    } else {
        0
    })
}

pub(crate) fn prepare_collaboration_worker_turn(
    runtime: &mut AgentLoopState,
    has_tools: bool,
    evidence_turn_limit: usize,
) -> bool {
    let finalizing = has_tools && runtime.turn >= evidence_turn_limit.max(1);
    if finalizing
        && runtime
            .messages
            .last()
            .and_then(|message| message.metadata.get("kind"))
            .map(String::as_str)
            != Some("collaboration_worker_finalization")
    {
        runtime.messages.push(Message {
            role: MessageRole::User,
            content: concat!(
                "The read-only evidence phase is complete and tools are now unavailable. ",
                "Do not request more tools. Return the assigned concise work product now, ",
                "grounded only in the evidence and context already collected."
            )
            .to_string(),
            metadata: [(
                "kind".to_string(),
                "collaboration_worker_finalization".to_string(),
            )]
            .into_iter()
            .collect(),
        });
    }
    finalizing
}
