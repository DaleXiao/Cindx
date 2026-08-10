use super::outcome_shadow::ShadowOutcomePairV1;
use super::workflow_gepa_campaign_contract::{CampaignSplit, ProductRunReceipt};
use super::workflow_gepa_campaign_execution::MatchedRoutePairRun;
use super::RawRun;
use agent_application::{
    CollaborationLearningArmOrderV1, CollaborationLearningArmV1,
    CollaborationLearningComparisonBindingV1, CollaborationLearningComparisonHashesV1,
    CollaborationLearningExerciseV1, CollaborationLearningPairV1, CollaborationLearningSplitV1,
    CollaborationLearningTrialV1,
};
use orchestrator::sha256_hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CollaborationLearningFrozenCaptureAuthority {
    source_commit_sha256: String,
    cohort_sha256: String,
}

impl CollaborationLearningFrozenCaptureAuthority {
    #[allow(dead_code)]
    pub(super) fn new(
        source_commit_sha256: impl Into<String>,
        cohort_sha256: impl Into<String>,
    ) -> Result<Self, String> {
        let authority = Self {
            source_commit_sha256: source_commit_sha256.into(),
            cohort_sha256: cohort_sha256.into(),
        };
        require_capture_sha256(&authority.source_commit_sha256, "source commit")?;
        require_capture_sha256(&authority.cohort_sha256, "cohort")?;
        Ok(authority)
    }
}

impl MatchedRoutePairRun {
    #[allow(dead_code)]
    pub(super) fn project_collaboration_learning_pair(
        &self,
        authority: &CollaborationLearningFrozenCaptureAuthority,
    ) -> Result<CollaborationLearningPairV1, String> {
        let outcomes = self.shadow_outcome_pair.as_ref().ok_or_else(|| {
            self.shadow_outcome_error.clone().unwrap_or_else(|| {
                "matched route pair is missing its externally verified outcome".to_string()
            })
        })?;
        let binding = self.collaboration_learning_binding(authority, outcomes)?;
        let direct_exercise = CollaborationLearningExerciseV1::from_events(
            &self.direct.collaboration_learning_events,
            &outcomes.direct,
        )
        .map_err(|error| error.to_string())?;
        let workflow_exercise = CollaborationLearningExerciseV1::from_events(
            &self.workflow.collaboration_learning_events,
            &outcomes.workflow,
        )
        .map_err(|error| error.to_string())?;
        let direct = CollaborationLearningTrialV1::new(
            binding.clone(),
            CollaborationLearningArmV1::Direct,
            outcomes.direct.clone(),
            direct_exercise,
        )
        .map_err(|error| error.to_string())?;
        let workflow = CollaborationLearningTrialV1::new(
            binding,
            CollaborationLearningArmV1::Workflow,
            outcomes.workflow.clone(),
            workflow_exercise,
        )
        .map_err(|error| error.to_string())?;
        CollaborationLearningPairV1::new(direct, workflow).map_err(|error| error.to_string())
    }

    fn collaboration_learning_binding(
        &self,
        authority: &CollaborationLearningFrozenCaptureAuthority,
        outcomes: &ShadowOutcomePairV1,
    ) -> Result<CollaborationLearningComparisonBindingV1, String> {
        validate_raw_capture_receipt(&self.direct, &self.pair.seed)?;
        validate_raw_capture_receipt(&self.workflow, &self.pair.candidate)?;
        let actual = matched_pair_capture_identity(&self.pair.seed, &self.pair.candidate)?;
        if self.direct.case_id != self.pair.case_id
            || self.workflow.case_id != self.pair.case_id
            || self.direct.replicate != self.pair.replicate
            || self.workflow.replicate != self.pair.replicate
            || self.direct.input_sha256 != self.workflow.input_sha256
            || self.direct.input_sha256 != outcomes.direct.verifier.subject_sha256
            || self.workflow.input_sha256 != outcomes.workflow.verifier.subject_sha256
            || outcomes.direct.resources.budget_sha256 != outcomes.workflow.resources.budget_sha256
        {
            return Err(
                "collaboration learning capture disagrees with the completed matched pair"
                    .to_string(),
            );
        }
        let direct_anchor = self
            .direct
            .strategy_receipt
            .as_ref()
            .and_then(|receipt| receipt.matched_route_plan_anchor.as_ref())
            .ok_or_else(|| {
                "collaboration learning capture is missing the Direct shared anchor".to_string()
            })?;
        let workflow_anchor = self
            .workflow
            .strategy_receipt
            .as_ref()
            .and_then(|receipt| receipt.matched_route_plan_anchor.as_ref())
            .ok_or_else(|| {
                "collaboration learning capture is missing the Workflow shared anchor".to_string()
            })?;
        if direct_anchor != workflow_anchor {
            return Err("collaboration learning capture changed the shared anchor".to_string());
        }
        let shared_conductor_anchor_sha256 = sha256_hex(
            &serde_json::to_vec(direct_anchor)
                .map_err(|error| format!("shared anchor encoding failed: {error}"))?,
        );
        let direct_models = canonical_configured_model_pool(&self.direct.configured_models)?;
        let workflow_models = canonical_configured_model_pool(&self.workflow.configured_models)?;
        if direct_models != workflow_models {
            return Err("collaboration learning capture changed the configured model pool".into());
        }
        let model_pool_sha256 = sha256_hex(
            &serde_json::to_vec(&direct_models)
                .map_err(|error| format!("model pool encoding failed: {error}"))?,
        );
        let replicate = u16::try_from(self.pair.replicate)
            .map_err(|_| "collaboration learning replicate exceeds its bound".to_string())?;
        let split = match self.pair.split {
            CampaignSplit::Train => CollaborationLearningSplitV1::Train,
            CampaignSplit::Validation | CampaignSplit::Test => {
                CollaborationLearningSplitV1::Holdout
            }
        };
        let arm_order = match self.pair.execution_order.as_str() {
            "forced_direct_then_forced_workflow" => CollaborationLearningArmOrderV1::DirectFirst,
            "forced_workflow_then_forced_direct" => CollaborationLearningArmOrderV1::WorkflowFirst,
            _ => return Err("collaboration learning capture arm order is invalid".to_string()),
        };
        CollaborationLearningComparisonBindingV1::freeze(
            CollaborationLearningComparisonHashesV1 {
                source_commit_sha256: authority.source_commit_sha256.clone(),
                suite_sha256: self.suite_sha256.clone(),
                case_sha256: self.direct.input_sha256.clone(),
                prestate_sha256: self.pair.workspace_prestate_sha256.clone(),
                provider_sha256: self.provider_sha256.clone(),
                model_pool_sha256,
                route_profile_sha256: actual.route_profile_sha256,
                prompt_profile_sha256: actual.prompt_profile_sha256,
                conductor_candidate_sha256: actual.conductor_candidate_sha256,
                workflow_proposal_sha256: actual.workflow_proposal_sha256,
                shared_conductor_anchor_sha256,
                direct_execution_plan_semantic_sha256: actual.direct_plan_sha256,
                workflow_execution_plan_semantic_sha256: actual.workflow_plan_sha256,
                budget_sha256: outcomes.direct.resources.budget_sha256.clone(),
                cohort_sha256: authority.cohort_sha256.clone(),
            },
            split,
            replicate,
            arm_order,
        )
        .map_err(|error| error.to_string())
    }
}

fn validate_raw_capture_receipt(run: &RawRun, receipt: &ProductRunReceipt) -> Result<(), String> {
    let strategy = run.strategy_receipt.as_ref().ok_or_else(|| {
        "collaboration learning capture is missing its strategy receipt".to_string()
    })?;
    if run.case_id != receipt.case_id
        || run.category != receipt.category
        || run.replicate != receipt.replicate
        || strategy.execution_mode != receipt.execution_mode
        || strategy.execution_constraint != receipt.execution_constraint.as_deref().unwrap_or("")
        || strategy.route_profile_sha256 != receipt.route_profile_sha256.as_deref().unwrap_or("")
        || strategy.profile_sha256 != receipt.profile_sha256.as_deref().unwrap_or("")
        || strategy.conductor_candidate_sha256.as_deref()
            != receipt.conductor_candidate_sha256.as_deref()
        || strategy.workflow_proposal_sha256.as_deref()
            != receipt.workflow_proposal_sha256.as_deref()
        || strategy.execution_plan_semantic_sha256.as_deref()
            != receipt.execution_plan_semantic_sha256.as_deref()
    {
        return Err(
            "collaboration learning capture receipt does not match its raw run".to_string(),
        );
    }
    Ok(())
}

struct MatchedPairCaptureIdentity {
    route_profile_sha256: String,
    prompt_profile_sha256: String,
    conductor_candidate_sha256: String,
    workflow_proposal_sha256: String,
    direct_plan_sha256: String,
    workflow_plan_sha256: String,
}

fn matched_pair_capture_identity(
    direct: &ProductRunReceipt,
    workflow: &ProductRunReceipt,
) -> Result<MatchedPairCaptureIdentity, String> {
    let route_profile_sha256 = validate_matched_receipt_field(
        direct.route_profile_sha256.as_deref(),
        workflow.route_profile_sha256.as_deref(),
        "route profile",
    )?;
    let prompt_profile_sha256 = validate_matched_receipt_field(
        direct.profile_sha256.as_deref(),
        workflow.profile_sha256.as_deref(),
        "prompt profile",
    )?;
    let conductor_candidate_sha256 = validate_matched_receipt_field(
        direct.conductor_candidate_sha256.as_deref(),
        workflow.conductor_candidate_sha256.as_deref(),
        "conductor candidate",
    )?;
    let workflow_proposal_sha256 = validate_matched_receipt_field(
        direct.workflow_proposal_sha256.as_deref(),
        workflow.workflow_proposal_sha256.as_deref(),
        "workflow proposal",
    )?;
    let direct_plan_sha256 = direct
        .execution_plan_semantic_sha256
        .as_deref()
        .ok_or_else(|| "collaboration learning capture is missing the Direct plan".to_string())?
        .to_string();
    let workflow_plan_sha256 = workflow
        .execution_plan_semantic_sha256
        .as_deref()
        .ok_or_else(|| "collaboration learning capture is missing the Workflow plan".to_string())?
        .to_string();
    require_capture_sha256(&direct_plan_sha256, "Direct plan")?;
    require_capture_sha256(&workflow_plan_sha256, "Workflow plan")?;
    if direct_plan_sha256 == workflow_plan_sha256 {
        return Err("collaboration learning capture treatments are identical".to_string());
    }
    Ok(MatchedPairCaptureIdentity {
        route_profile_sha256,
        prompt_profile_sha256,
        conductor_candidate_sha256,
        workflow_proposal_sha256,
        direct_plan_sha256,
        workflow_plan_sha256,
    })
}

fn validate_matched_receipt_field(
    direct: Option<&str>,
    workflow: Option<&str>,
    label: &str,
) -> Result<String, String> {
    let direct = direct
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("collaboration learning capture is missing the Direct {label}"))?;
    let workflow = workflow
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("collaboration learning capture is missing the Workflow {label}"))?;
    require_capture_sha256(direct, label)?;
    require_capture_sha256(workflow, label)?;
    if direct != workflow {
        return Err(format!(
            "collaboration learning capture changed the matched {label}"
        ));
    }
    Ok(direct.to_string())
}

fn canonical_configured_model_pool(models: &[String]) -> Result<Vec<String>, String> {
    let models = models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    if models.is_empty() {
        return Err("collaboration learning capture is missing its configured model pool".into());
    }
    Ok(models.into_iter().map(str::to_string).collect())
}

fn require_capture_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(format!(
            "collaboration learning capture {label} is not a SHA-256 digest"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_collaboration_learning_offline_adapter_contract_capture_requires_complete_actual_pair_identity(
    ) {
        let digest = "a".repeat(64);
        assert_eq!(
            validate_matched_receipt_field(Some(&digest), Some(&digest), "profile").unwrap(),
            digest
        );
        assert!(validate_matched_receipt_field(Some(&"a".repeat(64)), None, "profile").is_err());
        assert!(validate_matched_receipt_field(
            Some(&"a".repeat(64)),
            Some(&"b".repeat(64)),
            "profile"
        )
        .is_err());
        assert!(CollaborationLearningFrozenCaptureAuthority::new("bad", "c".repeat(64)).is_err());
    }
}
