use agent_core::Event;
use agent_runtime::{AgentRunControl, RunBudget};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PromptEvaluationCampaignUsage {
    pub(crate) started_at_ms: u64,
    pub(crate) total_tokens: u64,
    pub(crate) physical_model_attempts: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct PromptEvaluationCheckpoint {
    pub(crate) completed_actions: usize,
    pub(crate) campaign_usage: PromptEvaluationCampaignUsage,
    failed_closed: bool,
}

impl PromptEvaluationCheckpoint {
    pub(crate) fn parse(
        completed_actions: Option<&str>,
        campaign_usage: Option<&str>,
        checkpoint_at_ms: u64,
        previous: Option<&Self>,
    ) -> Self {
        if previous.is_some_and(|checkpoint| checkpoint.failed_closed) {
            return previous.cloned().expect("checked previous checkpoint");
        }
        let completed_actions = completed_actions.and_then(|value| value.parse::<usize>().ok());
        let campaign_usage =
            PromptEvaluationCampaignUsage::from_checkpoint(campaign_usage, checkpoint_at_ms);
        let valid = completed_actions
            .zip(campaign_usage.ok())
            .filter(|(actions, usage)| {
                previous.is_none_or(|previous| {
                    *actions >= previous.completed_actions
                        && usage.is_monotonic_successor_of(&previous.campaign_usage)
                })
            });
        match valid {
            Some((completed_actions, campaign_usage)) => Self {
                completed_actions,
                campaign_usage,
                failed_closed: false,
            },
            None => Self {
                completed_actions: completed_actions
                    .or_else(|| previous.map(|checkpoint| checkpoint.completed_actions))
                    .unwrap_or_default()
                    .max(
                        previous
                            .map(|checkpoint| checkpoint.completed_actions)
                            .unwrap_or_default(),
                    ),
                campaign_usage: PromptEvaluationCampaignUsage::fail_closed(),
                failed_closed: true,
            },
        }
    }

    pub(crate) fn is_valid_for_request_started_at(&self, started_at_ms: u64) -> bool {
        !self.failed_closed && self.campaign_usage.started_at_ms == started_at_ms
    }

    fn fail_closed(previous: Option<&Self>, minimum_actions: usize) -> Self {
        Self {
            completed_actions: previous
                .map(|checkpoint| checkpoint.completed_actions)
                .unwrap_or_default()
                .max(minimum_actions),
            campaign_usage: PromptEvaluationCampaignUsage::fail_closed(),
            failed_closed: true,
        }
    }
}

#[derive(Default)]
struct RecoveredProgress {
    checkpoint: Option<PromptEvaluationCheckpoint>,
    open_action: Option<(String, usize)>,
}

pub(crate) fn recover_prompt_evaluation_checkpoints(
    events: &[Event],
) -> Result<BTreeMap<String, PromptEvaluationCheckpoint>, String> {
    use crate::prompt_evolution_campaign_runtime::{
        ACTION_COMPLETED_EVENT, ACTION_ID_KEY, ACTION_INDEX_KEY, ACTION_STARTED_EVENT,
    };
    const CHECKPOINT_EVENT: &str = "Conductor prompt evaluation request checkpointed";
    const REQUEST_ID_KEY: &str = "prompt_evaluation_request_id";

    let mut ordered = events
        .iter()
        .filter(|event| {
            matches!(
                event.summary.as_str(),
                CHECKPOINT_EVENT | ACTION_STARTED_EVENT | ACTION_COMPLETED_EVENT
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_by_key(|event| event.sequence);
    let mut recovered = BTreeMap::<String, RecoveredProgress>::new();
    for event in ordered {
        let request_id = event
            .metadata
            .get(REQUEST_ID_KEY)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .ok_or_else(|| "prompt evaluation progress request id is invalid".to_string())?;
        let progress = recovered.entry(request_id).or_default();
        if progress
            .checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.failed_closed)
        {
            continue;
        }
        if event.summary == CHECKPOINT_EVENT {
            progress.checkpoint = Some(if progress.open_action.is_some() {
                PromptEvaluationCheckpoint::fail_closed(progress.checkpoint.as_ref(), 0)
            } else {
                PromptEvaluationCheckpoint::parse(
                    event.metadata.get("completed_actions").map(String::as_str),
                    event.metadata.get("campaign_usage").map(String::as_str),
                    event.timestamp_ms,
                    progress.checkpoint.as_ref(),
                )
            });
            continue;
        }
        let action_id = event.metadata.get(ACTION_ID_KEY).cloned();
        let action_index = event
            .metadata
            .get(ACTION_INDEX_KEY)
            .and_then(|value| value.parse::<usize>().ok());
        let valid_identity = action_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            && action_index.is_some();
        if event.summary == ACTION_STARTED_EVENT {
            let prior_actions = progress
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.completed_actions)
                .unwrap_or_default();
            if !valid_identity
                || progress.open_action.is_some()
                || action_index.is_some_and(|index| index < prior_actions)
            {
                progress.checkpoint = Some(PromptEvaluationCheckpoint::fail_closed(
                    progress.checkpoint.as_ref(),
                    action_index.unwrap_or(prior_actions).saturating_add(1),
                ));
            } else {
                progress.open_action = Some((action_id.unwrap(), action_index.unwrap()));
            }
            continue;
        }
        let matches_open = progress
            .open_action
            .as_ref()
            .is_some_and(|(open_id, open_index)| {
                Some(open_id) == action_id.as_ref() && Some(*open_index) == action_index
            });
        if !valid_identity || !matches_open {
            progress.checkpoint = Some(PromptEvaluationCheckpoint::fail_closed(
                progress.checkpoint.as_ref(),
                action_index.unwrap_or_default().saturating_add(1),
            ));
        } else {
            progress.checkpoint = Some(PromptEvaluationCheckpoint::parse(
                event.metadata.get("completed_actions").map(String::as_str),
                event.metadata.get("campaign_usage").map(String::as_str),
                event.timestamp_ms,
                progress.checkpoint.as_ref(),
            ));
            progress.open_action = None;
        }
    }
    Ok(recovered
        .into_iter()
        .filter_map(|(request_id, progress)| {
            let checkpoint = if let Some((_, action_index)) = progress.open_action {
                Some(PromptEvaluationCheckpoint::fail_closed(
                    progress.checkpoint.as_ref(),
                    action_index.saturating_add(1),
                ))
            } else {
                progress.checkpoint
            }?;
            Some((request_id, checkpoint))
        })
        .collect())
}

impl PromptEvaluationCampaignUsage {
    pub(crate) fn starting_at(started_at_ms: u64) -> Self {
        Self {
            started_at_ms,
            ..Self::default()
        }
    }

    pub(crate) fn from_checkpoint(
        encoded: Option<&str>,
        checkpoint_at_ms: u64,
    ) -> Result<Self, String> {
        let encoded = encoded.ok_or_else(|| "campaign usage is missing".to_string())?;
        let usage = serde_json::from_str::<Self>(encoded)
            .map_err(|error| format!("campaign usage is invalid: {error}"))?;
        if usage.started_at_ms == 0 || usage.started_at_ms > checkpoint_at_ms {
            return Err("campaign start time is invalid".to_string());
        }
        Ok(usage)
    }

    pub(crate) fn fail_closed() -> Self {
        Self {
            started_at_ms: 0,
            total_tokens: u64::MAX,
            physical_model_attempts: u64::MAX,
        }
    }

    pub(crate) fn is_monotonic_successor_of(&self, previous: &Self) -> bool {
        self.started_at_ms == previous.started_at_ms
            && self.total_tokens >= previous.total_tokens
            && self.physical_model_attempts >= previous.physical_model_attempts
    }

    pub(crate) fn with_control_usage(&self, control: &AgentRunControl) -> Self {
        let resources = control.resource_usage().segment;
        Self {
            started_at_ms: self.started_at_ms,
            total_tokens: self
                .total_tokens
                .saturating_add(resources.total_tokens)
                .saturating_add(resources.reserved_tokens),
            physical_model_attempts: self
                .physical_model_attempts
                .saturating_add(resources.physical_attempts),
        }
    }

    pub(crate) fn remaining_budget(&self, now_ms: u64) -> Option<RunBudget> {
        let mut budget = crate::prompt_learning_runtime::prompt_evaluation_parent_budget();
        let elapsed = Duration::from_millis(now_ms.saturating_sub(self.started_at_ms));
        let remaining_duration = budget.max_duration.saturating_sub(elapsed);
        let remaining_tokens = budget.max_total_tokens.saturating_sub(self.total_tokens);
        let used_attempts = usize::try_from(self.physical_model_attempts).unwrap_or(usize::MAX);
        let remaining_attempts = budget
            .max_physical_model_attempts
            .saturating_sub(used_attempts);
        if remaining_duration.is_zero() || remaining_tokens == 0 || remaining_attempts == 0 {
            return None;
        }
        budget.max_duration = remaining_duration;
        budget.max_total_tokens = remaining_tokens;
        budget.max_physical_model_attempts = remaining_attempts;
        budget.max_model_calls = budget.max_model_calls.min(remaining_attempts).max(1);
        budget.initial_model_calls = budget
            .initial_model_calls
            .min(budget.max_model_calls)
            .max(1);
        Some(budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::RunStageClass;

    #[test]
    fn campaign_budget_is_reduced_across_checkpoints() {
        let budget = crate::prompt_learning_runtime::prompt_evaluation_parent_budget();
        let usage = PromptEvaluationCampaignUsage {
            started_at_ms: 9_000,
            total_tokens: budget.max_total_tokens.saturating_sub(11),
            physical_model_attempts: u64::try_from(budget.max_physical_model_attempts)
                .unwrap()
                .saturating_sub(2),
        };

        let remaining = usage.remaining_budget(10_000).unwrap();

        assert_eq!(remaining.max_total_tokens, 11);
        assert_eq!(remaining.max_physical_model_attempts, 2);
        assert_eq!(
            remaining.max_duration,
            budget.max_duration.saturating_sub(Duration::from_secs(1))
        );
    }

    #[test]
    fn exhausted_campaign_cannot_restart_with_a_fresh_budget() {
        let budget = crate::prompt_learning_runtime::prompt_evaluation_parent_budget();
        let usage = PromptEvaluationCampaignUsage {
            started_at_ms: 1,
            total_tokens: budget.max_total_tokens,
            physical_model_attempts: 1,
        };

        assert!(usage.remaining_budget(2).is_none());
    }

    #[test]
    fn malformed_checkpoint_fails_closed_instead_of_resetting_usage() {
        let malformed = PromptEvaluationCampaignUsage::from_checkpoint(Some("not-json"), 10_000);
        let missing = PromptEvaluationCampaignUsage::from_checkpoint(None, 10_000);

        assert!(malformed.is_err());
        assert!(missing.is_err());
        assert!(PromptEvaluationCampaignUsage::fail_closed()
            .remaining_budget(10_000)
            .is_none());
    }

    #[test]
    fn campaign_usage_successor_requires_identical_start_and_monotonic_counters() {
        let previous = PromptEvaluationCampaignUsage {
            started_at_ms: 1,
            total_tokens: 42,
            physical_model_attempts: 3,
        };

        assert!(PromptEvaluationCampaignUsage {
            started_at_ms: 1,
            total_tokens: 43,
            physical_model_attempts: 4,
        }
        .is_monotonic_successor_of(&previous));
        assert!(!PromptEvaluationCampaignUsage {
            started_at_ms: 1,
            total_tokens: 41,
            physical_model_attempts: 4,
        }
        .is_monotonic_successor_of(&previous));
        assert!(!PromptEvaluationCampaignUsage {
            started_at_ms: 2,
            total_tokens: 43,
            physical_model_attempts: 4,
        }
        .is_monotonic_successor_of(&previous));
    }

    #[test]
    fn latest_control_usage_can_be_persisted_after_a_status_write_failure() {
        let control = AgentRunControl::with_budget(
            crate::prompt_learning_runtime::prompt_evaluation_parent_budget(),
        );
        let attempt = control
            .begin_physical_model_attempt("model", 1, 1, RunStageClass::Worker)
            .unwrap();
        assert!(control.finish_physical_model_attempt(attempt, None));
        let previous = PromptEvaluationCampaignUsage {
            started_at_ms: 1,
            total_tokens: 10,
            physical_model_attempts: 2,
        };

        let latest = previous.with_control_usage(&control);

        assert_eq!(latest.total_tokens, 12);
        assert_eq!(latest.physical_model_attempts, 3);
        assert!(latest.is_monotonic_successor_of(&previous));
    }
}
