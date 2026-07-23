use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PROMPT_EVOLUTION_CAMPAIGN_SCHEMA: &str = "cindx.prompt-evolution-campaign.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvolutionCampaignStage {
    NotApplicable,
    Disabled,
    CollectDataset,
    CollectFeedback,
    ReflectAndMutate,
    PairedTrain,
    HoldoutReplay,
    SelectFrontier,
    Canary,
    Stable,
    RolledBack,
    Frozen,
}

impl PromptEvolutionCampaignStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Disabled => "disabled",
            Self::CollectDataset => "collect_dataset",
            Self::CollectFeedback => "collect_feedback",
            Self::ReflectAndMutate => "reflect_and_mutate",
            Self::PairedTrain => "paired_train",
            Self::HoldoutReplay => "holdout_replay",
            Self::SelectFrontier => "select_frontier",
            Self::Canary => "canary",
            Self::Stable => "stable",
            Self::RolledBack => "rolled_back",
            Self::Frozen => "frozen",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEvolutionCampaignInput {
    pub effort: String,
    pub applicable: bool,
    pub enabled: bool,
    pub evaluation_inflight: bool,
    pub dataset_digest: String,
    pub dataset_cases: usize,
    pub minimum_dataset_cases: usize,
    pub reflection_packets: usize,
    pub learned_profiles: usize,
    pub paired_runs: usize,
    pub required_paired_runs: usize,
    pub replay_runs: usize,
    pub required_replay_runs: usize,
    pub ready_profiles: usize,
    pub stable_profile_id: String,
    pub canary_profile_id: Option<String>,
    pub canary_percent: u8,
    pub rollout_status: String,
    pub frozen: bool,
}

impl PromptEvolutionCampaignInput {
    pub fn validate(&self) -> Result<(), String> {
        if self.effort.trim().is_empty() {
            return Err("prompt evolution campaign effort is empty".to_string());
        }
        if self.minimum_dataset_cases == 0
            || self.required_paired_runs == 0
            || self.required_replay_runs == 0
        {
            return Err(
                "prompt evolution campaign evidence requirements must be positive".to_string(),
            );
        }
        if self.stable_profile_id.trim().is_empty() {
            return Err("prompt evolution campaign stable profile is empty".to_string());
        }
        if self.canary_percent > 100 {
            return Err("prompt evolution campaign canary percentage exceeds 100".to_string());
        }
        if self.canary_percent > 0 && self.canary_profile_id.is_none() {
            return Err(
                "prompt evolution campaign canary percentage requires a profile".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEvolutionCampaignSnapshot {
    pub schema: String,
    pub effort: String,
    pub stage: PromptEvolutionCampaignStage,
    pub next_action: String,
    pub resume_token: String,
    pub busy: bool,
}

impl PromptEvolutionCampaignSnapshot {
    pub fn readiness(&self) -> &'static str {
        if self.busy {
            return "evaluating";
        }
        match self.stage {
            PromptEvolutionCampaignStage::NotApplicable => "not_applicable",
            PromptEvolutionCampaignStage::Disabled => "disabled",
            PromptEvolutionCampaignStage::CollectDataset => "collecting_dataset",
            PromptEvolutionCampaignStage::CollectFeedback
            | PromptEvolutionCampaignStage::ReflectAndMutate
            | PromptEvolutionCampaignStage::PairedTrain => "collecting_train_evidence",
            PromptEvolutionCampaignStage::HoldoutReplay => "collecting_holdout_evidence",
            PromptEvolutionCampaignStage::SelectFrontier => "selecting_frontier",
            PromptEvolutionCampaignStage::Canary => "canary",
            PromptEvolutionCampaignStage::Stable
                if self.next_action == "monitor_promoted_profile" =>
            {
                "promoted"
            }
            PromptEvolutionCampaignStage::Stable => "ready",
            PromptEvolutionCampaignStage::RolledBack => "rolled_back",
            PromptEvolutionCampaignStage::Frozen => "ready",
        }
    }
}

pub fn derive_prompt_evolution_campaign(
    input: &PromptEvolutionCampaignInput,
) -> Result<PromptEvolutionCampaignSnapshot, String> {
    input.validate()?;
    let (stage, next_action) = if !input.applicable {
        (
            PromptEvolutionCampaignStage::NotApplicable,
            "use_single_model_path",
        )
    } else if !input.enabled {
        (
            PromptEvolutionCampaignStage::Disabled,
            "enable_prompt_evolution",
        )
    } else if input.rollout_status == "rolled_back" {
        (
            PromptEvolutionCampaignStage::RolledBack,
            "explore_after_rollback",
        )
    } else if input.rollout_status == "canary" {
        (PromptEvolutionCampaignStage::Canary, "monitor_canary")
    } else if input.rollout_status == "promoted" {
        (
            PromptEvolutionCampaignStage::Stable,
            "monitor_promoted_profile",
        )
    } else if input.frozen {
        (
            PromptEvolutionCampaignStage::Frozen,
            "monitor_frozen_frontier",
        )
    } else if input.dataset_cases < input.minimum_dataset_cases {
        (
            PromptEvolutionCampaignStage::CollectDataset,
            "collect_completed_tasks",
        )
    } else if input.reflection_packets == 0 {
        (
            PromptEvolutionCampaignStage::CollectFeedback,
            "collect_reflection_trajectories",
        )
    } else if input.learned_profiles == 0 {
        (
            PromptEvolutionCampaignStage::ReflectAndMutate,
            "mutate_from_reflections",
        )
    } else if input.paired_runs < input.required_paired_runs {
        (
            PromptEvolutionCampaignStage::PairedTrain,
            "run_paired_train",
        )
    } else if input.replay_runs < input.required_replay_runs {
        (
            PromptEvolutionCampaignStage::HoldoutReplay,
            "run_holdout_replay",
        )
    } else if input.ready_profiles == 0 {
        (
            PromptEvolutionCampaignStage::SelectFrontier,
            "select_pareto_frontier",
        )
    } else {
        (
            PromptEvolutionCampaignStage::Stable,
            "monitor_stable_profile",
        )
    };

    let mut identity = input.clone();
    identity.evaluation_inflight = false;
    let identity = serde_json::to_vec(&(PROMPT_EVOLUTION_CAMPAIGN_SCHEMA, identity))
        .map_err(|error| format!("prompt evolution campaign identity failed: {error}"))?;
    let resume_token = format!("campaign-{:x}", Sha256::digest(identity));

    Ok(PromptEvolutionCampaignSnapshot {
        schema: PROMPT_EVOLUTION_CAMPAIGN_SCHEMA.to_string(),
        effort: input.effort.clone(),
        stage,
        next_action: if input.evaluation_inflight {
            "wait_for_current_evaluation".to_string()
        } else {
            next_action.to_string()
        },
        resume_token,
        busy: input.evaluation_inflight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> PromptEvolutionCampaignInput {
        PromptEvolutionCampaignInput {
            effort: "auto".to_string(),
            applicable: true,
            enabled: true,
            evaluation_inflight: false,
            dataset_digest: "dataset-a".to_string(),
            dataset_cases: 3,
            minimum_dataset_cases: 3,
            reflection_packets: 1,
            learned_profiles: 1,
            paired_runs: 3,
            required_paired_runs: 3,
            replay_runs: 4,
            required_replay_runs: 4,
            ready_profiles: 1,
            stable_profile_id: "seed-auto-v1".to_string(),
            canary_profile_id: None,
            canary_percent: 0,
            rollout_status: "stable".to_string(),
            frozen: false,
        }
    }

    #[test]
    fn campaign_enforces_evidence_order() {
        let mut value = input();
        value.dataset_cases = 2;
        assert_eq!(
            derive_prompt_evolution_campaign(&value).unwrap().stage,
            PromptEvolutionCampaignStage::CollectDataset
        );
        value.dataset_cases = 3;
        value.reflection_packets = 0;
        assert_eq!(
            derive_prompt_evolution_campaign(&value).unwrap().stage,
            PromptEvolutionCampaignStage::CollectFeedback
        );
        value.reflection_packets = 1;
        value.learned_profiles = 0;
        assert_eq!(
            derive_prompt_evolution_campaign(&value).unwrap().stage,
            PromptEvolutionCampaignStage::ReflectAndMutate
        );
        value.learned_profiles = 1;
        value.paired_runs = 2;
        assert_eq!(
            derive_prompt_evolution_campaign(&value).unwrap().stage,
            PromptEvolutionCampaignStage::PairedTrain
        );
        value.paired_runs = 3;
        value.replay_runs = 3;
        assert_eq!(
            derive_prompt_evolution_campaign(&value).unwrap().stage,
            PromptEvolutionCampaignStage::HoldoutReplay
        );
    }

    #[test]
    fn campaign_resume_token_is_stable_across_inflight_recovery() {
        let value = input();
        let stable = derive_prompt_evolution_campaign(&value).unwrap();
        let mut inflight = value;
        inflight.evaluation_inflight = true;
        let resumed = derive_prompt_evolution_campaign(&inflight).unwrap();
        assert_eq!(stable.resume_token, resumed.resume_token);
        assert!(resumed.busy);
        assert_eq!(resumed.next_action, "wait_for_current_evaluation");
    }

    #[test]
    fn campaign_resume_token_changes_when_dataset_changes() {
        let value = input();
        let first = derive_prompt_evolution_campaign(&value).unwrap();
        let mut changed = value;
        changed.dataset_digest = "dataset-b".to_string();
        let second = derive_prompt_evolution_campaign(&changed).unwrap();
        assert_ne!(first.resume_token, second.resume_token);
    }

    #[test]
    fn rollout_safety_states_take_priority() {
        let mut value = input();
        value.rollout_status = "rolled_back".to_string();
        value.dataset_cases = 0;
        let rolled_back = derive_prompt_evolution_campaign(&value).unwrap();
        assert_eq!(rolled_back.stage, PromptEvolutionCampaignStage::RolledBack);

        value.rollout_status = "canary".to_string();
        value.canary_profile_id = Some("candidate".to_string());
        value.canary_percent = 25;
        let canary = derive_prompt_evolution_campaign(&value).unwrap();
        assert_eq!(canary.stage, PromptEvolutionCampaignStage::Canary);
    }

    #[test]
    fn campaign_rejects_invalid_canary_state() {
        let mut value = input();
        value.canary_percent = 10;
        assert!(derive_prompt_evolution_campaign(&value).is_err());
    }
}
