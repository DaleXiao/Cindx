use crate::{LearningEvidenceV1, OrchestrationPolicy, TaskClass};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingOutcome {
    Succeeded,
    Failed,
    UserRejected,
}

impl RoutingOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::UserRejected => "user_rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingTelemetry {
    pub task_class: TaskClass,
    pub context_signature: String,
    pub selected_policy: OrchestrationPolicy,
    pub selected_model: String,
    pub latency_ms: u64,
    pub outcome: RoutingOutcome,
    #[serde(default)]
    pub quality_score: Option<f32>,
    #[serde(default)]
    pub verification_passed: Option<bool>,
    #[serde(default)]
    pub learning_evidence: LearningEvidenceV1,
    pub cost_proxy: u64,
    pub tool_count: u64,
    pub retrieval_count: u64,
    pub user_override: bool,
}
