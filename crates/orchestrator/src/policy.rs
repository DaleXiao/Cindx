use agent_core::{Metadata, ModelRole};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationPolicy {
    Single,
    PlanExecuteReview,
    BestOfN { candidates: usize },
    AutoRouter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationStep {
    pub role: ModelRole,
    pub instruction: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationPlan {
    pub policy: OrchestrationPolicy,
    pub steps: Vec<OrchestrationStep>,
    pub metadata: Metadata,
}

impl OrchestrationPolicy {
    pub fn label(&self) -> &'static str {
        match self {
            OrchestrationPolicy::Single => "single",
            OrchestrationPolicy::PlanExecuteReview => "plan_execute_review",
            OrchestrationPolicy::BestOfN { .. } => "best_of_n",
            OrchestrationPolicy::AutoRouter => "auto_router",
        }
    }
}

pub fn parse_policy(label: &str) -> Option<OrchestrationPolicy> {
    match label {
        "single" => Some(OrchestrationPolicy::Single),
        "plan_execute_review" => Some(OrchestrationPolicy::PlanExecuteReview),
        "best_of_n" => Some(OrchestrationPolicy::BestOfN { candidates: 3 }),
        "auto_router" => Some(OrchestrationPolicy::AutoRouter),
        _ => None,
    }
}

pub fn role_label(role: &ModelRole) -> &'static str {
    match role {
        ModelRole::Planner => "planner",
        ModelRole::Executor => "executor",
        ModelRole::Reviewer => "reviewer",
        ModelRole::Summarizer => "summarizer",
        ModelRole::Embedder => "embedder",
    }
}

pub fn step_prompt(
    plan: &OrchestrationPlan,
    step_index: usize,
    user_prompt: &str,
    previous_outputs: &[String],
) -> Option<String> {
    let step = plan.steps.get(step_index)?;
    let mut prompt = String::new();
    prompt.push_str("You are running inside Cindx orchestration.\n");
    prompt.push_str(&format!("Policy: {}\n", plan.policy.label()));
    prompt.push_str(&format!("Role: {}\n", role_label(&step.role)));
    prompt.push_str(&format!("Step instruction: {}\n\n", step.instruction));
    prompt.push_str("User request:\n");
    prompt.push_str(user_prompt);
    prompt.push('\n');

    if !previous_outputs.is_empty() {
        prompt.push_str("\nPrevious step outputs:\n");
        for (index, output) in previous_outputs.iter().enumerate() {
            prompt.push_str(&format!("Step {}:\n{}\n", index + 1, output));
        }
    }

    Some(prompt)
}

pub fn default_plan(policy: OrchestrationPolicy) -> OrchestrationPlan {
    let steps = match policy {
        OrchestrationPolicy::Single => vec![OrchestrationStep {
            role: ModelRole::Executor,
            instruction: "Answer or act directly with tool support when needed.".to_string(),
            metadata: Metadata::new(),
        }],
        OrchestrationPolicy::PlanExecuteReview => vec![
            OrchestrationStep {
                role: ModelRole::Planner,
                instruction: "Create a concise, checkable plan.".to_string(),
                metadata: Metadata::new(),
            },
            OrchestrationStep {
                role: ModelRole::Executor,
                instruction: "Execute the approved plan through local tools.".to_string(),
                metadata: Metadata::new(),
            },
            OrchestrationStep {
                role: ModelRole::Reviewer,
                instruction: "Review the result against the request and evidence.".to_string(),
                metadata: Metadata::new(),
            },
        ],
        OrchestrationPolicy::BestOfN { candidates } => vec![
            OrchestrationStep {
                role: ModelRole::Planner,
                instruction: format!("Generate {candidates} independent candidate approaches."),
                metadata: Metadata::new(),
            },
            OrchestrationStep {
                role: ModelRole::Reviewer,
                instruction:
                    "Select or synthesize the best candidate using external evidence when possible."
                        .to_string(),
                metadata: Metadata::new(),
            },
        ],
        OrchestrationPolicy::AutoRouter => vec![OrchestrationStep {
            role: ModelRole::Executor,
            instruction: "Answer or act directly after the router selects a concrete policy."
                .to_string(),
            metadata: Metadata::new(),
        }],
    };

    OrchestrationPlan {
        policy,
        steps,
        metadata: Metadata::new(),
    }
}
