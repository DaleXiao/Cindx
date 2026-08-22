use crate::ModelRole;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationPolicy {
    Single,
    PlanExecuteReview,
    BestOfN { candidates: usize },
    AutoRouter,
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
