use crate::prompt_genome_types::PromptCommitStrategy;
use crate::{OrchestrationPolicy, TaskClass};
use serde::{Deserialize, Serialize};

pub const AUTO_COLLABORATION_MIN_UPLIFT_BPS: u16 = 3_000;
pub const AUTO_COLLABORATION_MIN_CONFIDENCE_BPS: u16 = 5_500;
pub const PRO_MIN_TEAM_UPLIFT_BPS: u16 = 250;
pub const MAX_PLANNING_STEPS: usize = 5;

fn default_max_workflow_steps() -> usize {
    MAX_PLANNING_STEPS
}

pub fn minimum_team_uplift_bps(effort: &str) -> u16 {
    if normalize_effort(effort) == "pro" {
        PRO_MIN_TEAM_UPLIFT_BPS
    } else {
        0
    }
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
    pub fn should_auto_collaborate(&self) -> bool {
        self.expected_uplift_bps >= AUTO_COLLABORATION_MIN_UPLIFT_BPS
            && self.confidence_bps >= AUTO_COLLABORATION_MIN_CONFIDENCE_BPS
    }

    pub fn with_prompt_commit_strategy(mut self, strategy: PromptCommitStrategy) -> Self {
        let task_quorum = self.min_successful_branches.max(1);
        self.stop_policy = match (self.effort.as_str(), strategy) {
            ("fast", _) => ConductorStopPolicy::FirstVerified,
            ("pro", PromptCommitStrategy::Exhaustive) => ConductorStopPolicy::Exhaustive,
            ("pro", _) => ConductorStopPolicy::Quorum,
            (_, PromptCommitStrategy::Adaptive) => self.stop_policy,
            (_, PromptCommitStrategy::Quorum) => ConductorStopPolicy::Quorum,
            (_, PromptCommitStrategy::Exhaustive) => ConductorStopPolicy::Exhaustive,
        };
        self.min_successful_branches = match self.stop_policy {
            ConductorStopPolicy::FirstVerified => 1,
            ConductorStopPolicy::Quorum => task_quorum,
            ConductorStopPolicy::Exhaustive => {
                task_quorum.max(self.max_parallelism.saturating_mul(2).saturating_add(2) / 3)
            }
        };
        self
    }

    pub fn required_successes_for_layer(&self, branch_count: usize) -> usize {
        self.min_successful_branches.min(branch_count).max(1)
    }

    pub fn quorum_grace_ms(&self) -> u64 {
        match self.stop_policy {
            ConductorStopPolicy::FirstVerified => 100,
            ConductorStopPolicy::Quorum => 1_000,
            ConductorStopPolicy::Exhaustive => 20_000,
        }
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("execution contract serialization failed: {error}"))
    }

    pub fn from_json(value: &str) -> Result<Self, String> {
        serde_json::from_str(value)
            .map_err(|error| format!("execution contract JSON is invalid: {error}"))
    }
}

pub fn minimum_workflow_steps(distinct_contributions: usize, verification_required: bool) -> usize {
    if distinct_contributions == 0 {
        1
    } else {
        distinct_contributions
            .saturating_add(1)
            .saturating_add(usize::from(verification_required))
            .min(MAX_PLANNING_STEPS)
    }
}

pub fn normalize_effort(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "fast" => "fast".to_string(),
        "pro" => "pro".to_string(),
        _ => "auto".to_string(),
    }
}
