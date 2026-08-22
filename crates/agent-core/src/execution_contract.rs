use crate::{OrchestrationPolicy, TaskClass};
use serde::{Deserialize, Serialize};

pub const MAX_PLANNING_STEPS: usize = 5;

fn default_max_workflow_steps() -> usize {
    MAX_PLANNING_STEPS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConductorStopPolicy {
    FirstVerified,
    Quorum,
    Exhaustive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConductorFallbackPolicy {
    BestKnownResult,
    SinglePath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConductorExecutionContract {
    pub task_class: TaskClass,
    pub effort: String,
    pub policy: OrchestrationPolicy,
    pub expected_uplift_bps: u16,
    pub confidence_bps: u16,
    pub max_parallelism: usize,
    #[serde(default = "default_max_workflow_steps")]
    pub max_workflow_steps: usize,
    pub min_successful_branches: usize,
    pub verification_required: bool,
    pub terminal_model_call_reserve: usize,
    pub stop_policy: ConductorStopPolicy,
    pub fallback_policy: ConductorFallbackPolicy,
    #[serde(default)]
    pub min_team_uplift_bps: u16,
    #[serde(default)]
    pub min_distinct_contributions: usize,
    #[serde(default)]
    pub requires_synthesis: bool,
}

impl ConductorExecutionContract {
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("execution contract serialization failed: {error}"))
    }

    pub fn from_json(value: &str) -> Result<Self, String> {
        serde_json::from_str(value)
            .map_err(|error| format!("execution contract JSON is invalid: {error}"))
    }
}
