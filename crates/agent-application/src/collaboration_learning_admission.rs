use crate::collaboration_learning_policy::{
    collaboration_learning_sha256, validate_collaboration_learning_sha256,
    CollaborationLearningError, CollaborationLearningPolicyV1, CollaborationPolicyAxisV1,
    CollaborationRepairV1, CollaborationSpecialistInvocationV1,
};
use crate::collaboration_learning_projection::CollaborationLearningExerciseV1;
use crate::{AgentOutcomeUsageCompletenessV1, ExternallyVerifiedOutcomeV1};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

type ContractResult<T> = Result<T, CollaborationLearningError>;

pub const COLLABORATION_LEARNING_COMPARISON_SCHEMA: &str =
    "cindx.agent-collaboration-learning-comparison.v1";
pub const COLLABORATION_LEARNING_CANDIDATE_SCHEMA: &str =
    "cindx.agent-collaboration-learning-candidate.v1";
pub const COLLABORATION_LEARNING_CONFIG_SCHEMA: &str =
    "cindx.agent-collaboration-learning-config.v1";
pub const COLLABORATION_LEARNING_REVIEW_SCHEMA: &str =
    "cindx.agent-collaboration-learning-review.v1";
pub const COLLABORATION_LEARNING_OFFLINE_ADMISSION_SCHEMA: &str =
    "cindx.agent-collaboration-learning-offline-admission.v1";

const COMPARISON_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-comparison.v1\0";
const PAIR_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-pair.v1\0";
const CANDIDATE_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-candidate.v1\0";
const HOLDOUT_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-holdout.v1\0";
const SCOPE_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-scope.v1\0";
const CENSOR_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-censor.v1\0";
const CONFIG_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-config.v1\0";
const EVIDENCE_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-evidence.v1\0";
const REVIEW_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-review.v1\0";
const ADMISSION_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-offline-admission.v1\0";

const MAX_COMPARISONS: usize = 256;
const MAX_CANDIDATES: u16 = 8;
const MAX_RESOURCE_REGRESSION_BPS: u16 = 2_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningSplitV1 {
    Train,
    Holdout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningArmOrderV1 {
    DirectFirst,
    WorkflowFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningArmV1 {
    Direct,
    Workflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollaborationLearningComparisonHashesV1 {
    pub source_commit_sha256: String,
    pub suite_sha256: String,
    pub case_sha256: String,
    pub prestate_sha256: String,
    pub provider_sha256: String,
    pub model_pool_sha256: String,
    pub route_profile_sha256: String,
    pub prompt_profile_sha256: String,
    pub conductor_candidate_sha256: String,
    pub workflow_proposal_sha256: String,
    pub shared_conductor_anchor_sha256: String,
    pub direct_execution_plan_semantic_sha256: String,
    pub workflow_execution_plan_semantic_sha256: String,
    pub budget_sha256: String,
    pub cohort_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollaborationLearningComparisonBindingV1 {
    hashes: CollaborationLearningComparisonHashesV1,
    split: CollaborationLearningSplitV1,
    replicate: u16,
    arm_order: CollaborationLearningArmOrderV1,
    binding_sha256: String,
}

impl CollaborationLearningComparisonBindingV1 {
    pub fn freeze(
        hashes: CollaborationLearningComparisonHashesV1,
        split: CollaborationLearningSplitV1,
        replicate: u16,
        arm_order: CollaborationLearningArmOrderV1,
    ) -> ContractResult<Self> {
        for (value, label) in [
            (&hashes.source_commit_sha256, "source commit"),
            (&hashes.suite_sha256, "suite"),
            (&hashes.case_sha256, "case"),
            (&hashes.prestate_sha256, "prestate"),
            (&hashes.provider_sha256, "provider"),
            (&hashes.model_pool_sha256, "model pool"),
            (&hashes.route_profile_sha256, "route profile"),
            (&hashes.prompt_profile_sha256, "prompt profile"),
            (&hashes.conductor_candidate_sha256, "conductor candidate"),
            (&hashes.workflow_proposal_sha256, "workflow proposal"),
            (
                &hashes.shared_conductor_anchor_sha256,
                "shared conductor anchor",
            ),
            (
                &hashes.direct_execution_plan_semantic_sha256,
                "Direct execution plan",
            ),
            (
                &hashes.workflow_execution_plan_semantic_sha256,
                "Workflow execution plan",
            ),
            (&hashes.budget_sha256, "budget"),
            (&hashes.cohort_sha256, "cohort"),
        ] {
            validate_collaboration_learning_sha256(value, label)?;
        }
        if hashes.direct_execution_plan_semantic_sha256
            == hashes.workflow_execution_plan_semantic_sha256
        {
            return Err(error(
                "Direct and Workflow execution plans must be distinct treatments",
            ));
        }
        if replicate == 0 || usize::from(replicate) > MAX_COMPARISONS {
            return Err(error("comparison replicate is outside its fixed bound"));
        }
        let mut binding = Self {
            hashes,
            split,
            replicate,
            arm_order,
            binding_sha256: String::new(),
        };
        binding.binding_sha256 = binding.payload_sha256()?;
        Ok(binding)
    }

    fn payload_sha256(&self) -> ContractResult<String> {
        collaboration_learning_sha256(
            COMPARISON_HASH_DOMAIN,
            &(
                COLLABORATION_LEARNING_COMPARISON_SCHEMA,
                &self.hashes,
                self.split,
                self.replicate,
                self.arm_order,
            ),
            "comparison binding",
        )
    }

    fn validate(&self) -> ContractResult<()> {
        if self.binding_sha256 != self.payload_sha256()? {
            return Err(error("comparison binding digest is invalid"));
        }
        Ok(())
    }

    pub fn split(&self) -> CollaborationLearningSplitV1 {
        self.split
    }

    pub fn arm_order(&self) -> CollaborationLearningArmOrderV1 {
        self.arm_order
    }

    pub fn digest(&self) -> &str {
        &self.binding_sha256
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationLearningTrialV1 {
    binding: CollaborationLearningComparisonBindingV1,
    arm: CollaborationLearningArmV1,
    outcome: ExternallyVerifiedOutcomeV1,
    exercise: CollaborationLearningExerciseV1,
}

impl CollaborationLearningTrialV1 {
    pub fn new(
        binding: CollaborationLearningComparisonBindingV1,
        arm: CollaborationLearningArmV1,
        outcome: ExternallyVerifiedOutcomeV1,
        exercise: CollaborationLearningExerciseV1,
    ) -> ContractResult<Self> {
        binding.validate()?;
        outcome
            .validate()
            .map_err(|cause| error(format!("outcome is invalid: {cause}")))?;
        let expected_plan_sha256 = match arm {
            CollaborationLearningArmV1::Direct => {
                &binding.hashes.direct_execution_plan_semantic_sha256
            }
            CollaborationLearningArmV1::Workflow => {
                &binding.hashes.workflow_execution_plan_semantic_sha256
            }
        };
        if outcome.lifecycle.execution_plan_semantic_sha256 != *expected_plan_sha256
            || outcome.resources.budget_sha256 != binding.hashes.budget_sha256
            || outcome.verifier.subject_sha256 != binding.hashes.case_sha256
            || exercise.assignment.case_binding_sha256 != binding.hashes.case_sha256
        {
            return Err(error(
                "trial is not bound to its frozen plan, case, and budget",
            ));
        }
        let policy = exercise.assignment.policy()?;
        exercise.validate_against(&policy, &outcome)?;
        let topology = match arm {
            CollaborationLearningArmV1::Direct => {
                CollaborationSpecialistInvocationV1::DirectOwnerOnly
            }
            CollaborationLearningArmV1::Workflow => {
                CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
            }
        };
        if policy.specialist_invocation != topology {
            return Err(error("trial policy does not match its frozen arm"));
        }
        Ok(Self {
            binding,
            arm,
            outcome,
            exercise,
        })
    }

    pub fn reward_bps(&self) -> ContractResult<u16> {
        self.outcome
            .reward_bps()
            .map_err(|cause| error(format!("reward is invalid: {cause}")))
    }

    fn policy(&self) -> ContractResult<CollaborationLearningPolicyV1> {
        self.exercise.assignment.policy()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationLearningPairV1 {
    binding: CollaborationLearningComparisonBindingV1,
    direct: CollaborationLearningTrialV1,
    workflow: CollaborationLearningTrialV1,
    pair_sha256: String,
}

impl CollaborationLearningPairV1 {
    pub fn new(
        direct: CollaborationLearningTrialV1,
        workflow: CollaborationLearningTrialV1,
    ) -> ContractResult<Self> {
        if direct.arm != CollaborationLearningArmV1::Direct
            || workflow.arm != CollaborationLearningArmV1::Workflow
            || direct.binding != workflow.binding
        {
            return Err(error("pair arms do not share one frozen comparison"));
        }
        if direct.outcome.verifier.kind != workflow.outcome.verifier.kind
            || direct.outcome.verifier.protocol_sha256 != workflow.outcome.verifier.protocol_sha256
            || direct.outcome.verifier.subject_sha256 != workflow.outcome.verifier.subject_sha256
            || direct.outcome.resources.budget_sha256 != workflow.outcome.resources.budget_sha256
            || postcondition_contract(&direct.outcome) != postcondition_contract(&workflow.outcome)
            || direct.outcome.lifecycle.terminal_commit_key
                == workflow.outcome.lifecycle.terminal_commit_key
        {
            return Err(error(
                "pair changed an external verifier, postcondition, or budget confound",
            ));
        }
        validate_direct_exposure(&direct.outcome)?;
        validate_workflow_exposure(&workflow.outcome)?;
        let binding = direct.binding.clone();
        let pair_sha256 = collaboration_learning_sha256(
            PAIR_HASH_DOMAIN,
            &(
                binding.digest(),
                &direct.outcome,
                &direct.exercise,
                &workflow.outcome,
                &workflow.exercise,
            ),
            "matched pair",
        )?;
        Ok(Self {
            binding,
            direct,
            workflow,
            pair_sha256,
        })
    }

    pub fn binding(&self) -> &CollaborationLearningComparisonBindingV1 {
        &self.binding
    }

    pub fn digest(&self) -> &str {
        &self.pair_sha256
    }

    pub fn direct_reward_bps(&self) -> ContractResult<u16> {
        self.direct.reward_bps()
    }

    pub fn workflow_reward_bps(&self) -> ContractResult<u16> {
        self.workflow.reward_bps()
    }

    fn workflow_policy(&self) -> ContractResult<CollaborationLearningPolicyV1> {
        self.workflow.policy()
    }
}

fn postcondition_contract(
    outcome: &ExternallyVerifiedOutcomeV1,
) -> BTreeSet<(bool, &str, &str, &str)> {
    outcome
        .postconditions
        .iter()
        .map(|receipt| {
            (
                receipt.preservation,
                receipt.kind.as_str(),
                receipt.subject_sha256.as_str(),
                receipt.expected_sha256.as_str(),
            )
        })
        .collect()
}

fn validate_direct_exposure(outcome: &ExternallyVerifiedOutcomeV1) -> ContractResult<()> {
    let exposure = &outcome.exposure;
    if exposure.worker_model_calls != 0
        || exposure.successful_specialist_model_calls != 0
        || exposure.successful_independent_verifier_model_calls != 0
        || exposure.workflow_planned
        || exposure.workflow_completed
    {
        return Err(error("Direct arm exposed a workflow worker"));
    }
    Ok(())
}

fn validate_workflow_exposure(outcome: &ExternallyVerifiedOutcomeV1) -> ContractResult<()> {
    let exposure = &outcome.exposure;
    if !exposure.workflow_planned
        || exposure.worker_model_calls == 0
        || exposure.direct_anchor_competition_calls > 0
        || exposure.non_owner_permission_gated_calls > 0
    {
        return Err(error(
            "Workflow arm did not exercise its assigned read-only worker",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationLearningCandidateV1 {
    policy: CollaborationLearningPolicyV1,
    changed_axis: Option<CollaborationPolicyAxisV1>,
    holdout_manifest_sha256: String,
    config_sha256: String,
    proposer_identity_sha256: String,
    ordinal: u16,
    candidate_sha256: String,
}

impl CollaborationLearningCandidateV1 {
    pub fn snapshot(
        policy: CollaborationLearningPolicyV1,
        parent: Option<CollaborationLearningPolicyV1>,
        holdout_manifest_sha256: String,
        config_sha256: String,
        proposer_identity_sha256: String,
        ordinal: u16,
    ) -> ContractResult<Self> {
        policy.validate()?;
        validate_collaboration_learning_sha256(&holdout_manifest_sha256, "holdout manifest")?;
        validate_collaboration_learning_sha256(&config_sha256, "admission config")?;
        validate_collaboration_learning_sha256(&proposer_identity_sha256, "candidate proposer")?;
        if ordinal == 0 || ordinal > MAX_CANDIDATES {
            return Err(error("candidate ordinal is outside its fixed bound"));
        }
        if policy.specialist_invocation
            != CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
        {
            return Err(error("candidate cannot change the Goal 2 topology"));
        }
        let changed_axis = match parent {
            Some(parent) => Some(policy.changed_axis_against(&parent)?),
            None if policy.parent_policy_sha256.is_none() => None,
            None => return Err(error("candidate parent is missing")),
        };
        let mut snapshot = Self {
            policy,
            changed_axis,
            holdout_manifest_sha256,
            config_sha256,
            proposer_identity_sha256,
            ordinal,
            candidate_sha256: String::new(),
        };
        snapshot.candidate_sha256 = collaboration_learning_sha256(
            CANDIDATE_HASH_DOMAIN,
            &(
                COLLABORATION_LEARNING_CANDIDATE_SCHEMA,
                &snapshot.policy,
                snapshot.changed_axis,
                snapshot.holdout_manifest_sha256.as_str(),
                snapshot.config_sha256.as_str(),
                snapshot.proposer_identity_sha256.as_str(),
                snapshot.ordinal,
            ),
            "candidate snapshot",
        )?;
        Ok(snapshot)
    }

    pub fn digest(&self) -> &str {
        &self.candidate_sha256
    }

    pub fn policy(&self) -> &CollaborationLearningPolicyV1 {
        &self.policy
    }

    pub fn freeze_holdout_manifest(
        bindings: &[CollaborationLearningComparisonBindingV1],
    ) -> ContractResult<String> {
        Ok(freeze_holdout(bindings)?.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningCensorReasonV1 {
    InvalidOutcome,
    InvalidExercise,
    InvalidBinding,
    IncompleteInstrumentation,
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationLearningCensorReceiptV1 {
    binding: CollaborationLearningComparisonBindingV1,
    source_sha256: String,
    reason: CollaborationLearningCensorReasonV1,
    receipt_sha256: String,
}

impl CollaborationLearningCensorReceiptV1 {
    pub fn new(
        binding: CollaborationLearningComparisonBindingV1,
        source_sha256: String,
        reason: CollaborationLearningCensorReasonV1,
    ) -> ContractResult<Self> {
        validate_collaboration_learning_sha256(&source_sha256, "censor source")?;
        let receipt_sha256 = collaboration_learning_sha256(
            CENSOR_HASH_DOMAIN,
            &(binding.digest(), source_sha256.as_str(), reason),
            "censor receipt",
        )?;
        Ok(Self {
            binding,
            source_sha256,
            reason,
            receipt_sha256,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationLearningConfigV1 {
    min_train_pairs: u16,
    min_holdout_pairs: u16,
    minimum_uplift_bps: u16,
    max_resource_regression_bps: u16,
    max_position_imbalance: u16,
    candidate_budget: u16,
    config_sha256: String,
}

impl CollaborationLearningConfigV1 {
    pub fn freeze(
        min_train_pairs: u16,
        min_holdout_pairs: u16,
        minimum_uplift_bps: u16,
        max_resource_regression_bps: u16,
        max_position_imbalance: u16,
        candidate_budget: u16,
    ) -> ContractResult<Self> {
        if min_train_pairs == 0
            || min_holdout_pairs == 0
            || usize::from(min_train_pairs) + usize::from(min_holdout_pairs) > MAX_COMPARISONS
            || minimum_uplift_bps == 0
            || minimum_uplift_bps > 10_000
            || max_resource_regression_bps > MAX_RESOURCE_REGRESSION_BPS
            || max_position_imbalance > 1
            || candidate_budget == 0
            || candidate_budget > MAX_CANDIDATES
        {
            return Err(error("admission config is outside its fixed bounds"));
        }
        let mut config = Self {
            min_train_pairs,
            min_holdout_pairs,
            minimum_uplift_bps,
            max_resource_regression_bps,
            max_position_imbalance,
            candidate_budget,
            config_sha256: String::new(),
        };
        config.config_sha256 = collaboration_learning_sha256(
            CONFIG_HASH_DOMAIN,
            &(
                COLLABORATION_LEARNING_CONFIG_SCHEMA,
                config.min_train_pairs,
                config.min_holdout_pairs,
                config.minimum_uplift_bps,
                config.max_resource_regression_bps,
                config.max_position_imbalance,
                config.candidate_budget,
            ),
            "admission config",
        )?;
        Ok(config)
    }

    pub fn digest(&self) -> &str {
        &self.config_sha256
    }
}

fn freeze_holdout(
    bindings: &[CollaborationLearningComparisonBindingV1],
) -> ContractResult<(String, BTreeSet<String>, BTreeSet<String>)> {
    if bindings.is_empty() || bindings.len() > MAX_COMPARISONS {
        return Err(error("holdout must be frozen before candidate proposal"));
    }
    let mut digests = BTreeSet::new();
    let mut cases = BTreeSet::new();
    for binding in bindings {
        binding.validate()?;
        if binding.split() != CollaborationLearningSplitV1::Holdout
            || !digests.insert(binding.digest().to_string())
        {
            return Err(error("frozen holdout is invalid or duplicated"));
        }
        cases.insert(binding.hashes.case_sha256.clone());
    }
    let manifest =
        collaboration_learning_sha256(HOLDOUT_HASH_DOMAIN, &digests, "holdout manifest")?;
    Ok((manifest, digests, cases))
}

fn comparison_scope_sha256(
    binding: &CollaborationLearningComparisonBindingV1,
) -> ContractResult<String> {
    binding.validate()?;
    let hashes = &binding.hashes;
    collaboration_learning_sha256(
        SCOPE_HASH_DOMAIN,
        &(
            hashes.source_commit_sha256.as_str(),
            hashes.suite_sha256.as_str(),
            hashes.provider_sha256.as_str(),
            hashes.model_pool_sha256.as_str(),
            hashes.route_profile_sha256.as_str(),
            hashes.prompt_profile_sha256.as_str(),
            hashes.budget_sha256.as_str(),
            hashes.cohort_sha256.as_str(),
        ),
        "comparison scope",
    )
}

#[derive(Debug, Clone, Serialize)]
enum CollaborationLearningObservationV1 {
    Pair(Box<CollaborationLearningPairV1>),
    Censored(Box<CollaborationLearningCensorReceiptV1>),
}

#[derive(Debug, Clone)]
pub struct CollaborationLearningEvidenceSetV1 {
    candidate: CollaborationLearningCandidateV1,
    baseline_pair_sha256: String,
    comparison_scope_sha256: String,
    frozen_holdout: BTreeSet<String>,
    frozen_holdout_cases: BTreeSet<String>,
    observed_terminal_commit_keys: BTreeSet<String>,
    observations: BTreeMap<String, CollaborationLearningObservationV1>,
    config: CollaborationLearningConfigV1,
    sealed: bool,
}

impl CollaborationLearningEvidenceSetV1 {
    pub fn new(
        candidate: CollaborationLearningCandidateV1,
        baseline: CollaborationLearningPairV1,
        config: CollaborationLearningConfigV1,
        frozen_holdout: Vec<CollaborationLearningComparisonBindingV1>,
    ) -> ContractResult<Self> {
        let baseline_policy = baseline.workflow_policy()?;
        if baseline.binding().split() != CollaborationLearningSplitV1::Train
            || candidate.changed_axis.is_none()
            || baseline_policy.parent_policy_sha256.is_some()
            || candidate.policy.parent_policy_sha256.as_deref()
                != Some(baseline_policy.policy_sha256.as_str())
            || !complete_usage(&baseline)
            || has_safety_or_preservation_failure(&baseline)
            || exceeds_resource_regression(&baseline, config.max_resource_regression_bps)
            || baseline.workflow_reward_bps()? <= baseline.direct_reward_bps()?
        {
            return Err(error("candidate lacks a valid positive seed baseline"));
        }
        let (holdout_manifest_sha256, frozen, frozen_cases) = freeze_holdout(&frozen_holdout)?;
        let scope_sha256 = comparison_scope_sha256(baseline.binding())?;
        if candidate.holdout_manifest_sha256 != holdout_manifest_sha256
            || candidate.config_sha256 != config.config_sha256
            || frozen_cases.contains(&baseline.binding.hashes.case_sha256)
            || frozen_holdout.iter().any(|binding| {
                comparison_scope_sha256(binding).as_deref() != Ok(scope_sha256.as_str())
            })
        {
            return Err(error(
                "candidate or baseline does not match the precommitted holdout",
            ));
        }
        Ok(Self {
            candidate,
            baseline_pair_sha256: baseline.digest().to_string(),
            comparison_scope_sha256: scope_sha256,
            frozen_holdout: frozen,
            frozen_holdout_cases: frozen_cases,
            observed_terminal_commit_keys: BTreeSet::from([
                baseline
                    .direct
                    .outcome
                    .lifecycle
                    .terminal_commit_key
                    .clone(),
                baseline
                    .workflow
                    .outcome
                    .lifecycle
                    .terminal_commit_key
                    .clone(),
            ]),
            observations: BTreeMap::new(),
            config,
            sealed: false,
        })
    }

    pub fn append_pair(&mut self, pair: CollaborationLearningPairV1) -> ContractResult<()> {
        if pair.workflow_policy()?.policy_sha256 != self.candidate.policy.policy_sha256 {
            return Err(error("pair is not assigned to this candidate"));
        }
        if !changed_axis_exercised(&self.candidate, &pair) {
            return Err(error("candidate changed axis was not actually exercised"));
        }
        self.append_observation(
            pair.binding().clone(),
            CollaborationLearningObservationV1::Pair(Box::new(pair)),
        )
    }

    pub fn append_censor(
        &mut self,
        censor: CollaborationLearningCensorReceiptV1,
    ) -> ContractResult<()> {
        self.append_observation(
            censor.binding.clone(),
            CollaborationLearningObservationV1::Censored(Box::new(censor)),
        )
    }

    fn append_observation(
        &mut self,
        binding: CollaborationLearningComparisonBindingV1,
        observation: CollaborationLearningObservationV1,
    ) -> ContractResult<()> {
        if self.sealed {
            return Err(error("evidence set is sealed"));
        }
        binding.validate()?;
        if comparison_scope_sha256(&binding)? != self.comparison_scope_sha256 {
            return Err(error("observation changed the frozen comparison scope"));
        }
        let key = binding.digest().to_string();
        if binding.split() == CollaborationLearningSplitV1::Holdout {
            if !self.frozen_holdout.contains(&key) {
                return Err(error("observation is outside the frozen holdout"));
            }
        } else if self
            .frozen_holdout_cases
            .contains(&binding.hashes.case_sha256)
        {
            return Err(error("train and holdout identities overlap"));
        }
        if self.observations.len() >= MAX_COMPARISONS || self.observations.contains_key(&key) {
            return Err(error("observation is duplicated or exceeds its bound"));
        }
        let terminal_commit_keys = match &observation {
            CollaborationLearningObservationV1::Pair(pair) => Some([
                pair.direct.outcome.lifecycle.terminal_commit_key.clone(),
                pair.workflow.outcome.lifecycle.terminal_commit_key.clone(),
            ]),
            CollaborationLearningObservationV1::Censored(_) => None,
        };
        if terminal_commit_keys.as_ref().is_some_and(|keys| {
            keys[0] == keys[1]
                || keys
                    .iter()
                    .any(|receipt| self.observed_terminal_commit_keys.contains(receipt))
        }) {
            return Err(error(
                "one physical run was replayed as multiple observations",
            ));
        }
        self.observations.insert(key, observation);
        if let Some(keys) = terminal_commit_keys {
            self.observed_terminal_commit_keys.extend(keys);
        }
        Ok(())
    }

    pub fn training_pairs(&self) -> impl Iterator<Item = &CollaborationLearningPairV1> {
        self.observations
            .values()
            .filter_map(|observation| match observation {
                CollaborationLearningObservationV1::Pair(pair)
                    if pair.binding().split() == CollaborationLearningSplitV1::Train =>
                {
                    Some(pair.as_ref())
                }
                _ => None,
            })
    }

    pub fn censored_count(&self) -> usize {
        self.observations
            .values()
            .filter(|value| matches!(value, CollaborationLearningObservationV1::Censored(_)))
            .count()
    }

    pub fn aggregate(&mut self) -> ContractResult<CollaborationLearningAggregateV1> {
        if self.sealed {
            return Err(error("evidence set is sealed"));
        }
        let config = &self.config;
        let evidence_sha256 = collaboration_learning_sha256(
            EVIDENCE_HASH_DOMAIN,
            &(
                self.candidate.digest(),
                self.baseline_pair_sha256.as_str(),
                self.comparison_scope_sha256.as_str(),
                &self.frozen_holdout,
                &self.frozen_holdout_cases,
                &self.observed_terminal_commit_keys,
                &self.observations,
                config.config_sha256.as_str(),
            ),
            "evidence set",
        )?;
        let pairs = self
            .observations
            .values()
            .filter_map(|value| match value {
                CollaborationLearningObservationV1::Pair(pair) => Some(pair.as_ref()),
                CollaborationLearningObservationV1::Censored(_) => None,
            })
            .collect::<Vec<_>>();
        let train = pairs
            .iter()
            .copied()
            .filter(|pair| pair.binding().split() == CollaborationLearningSplitV1::Train)
            .collect::<Vec<_>>();
        let holdout = pairs
            .iter()
            .copied()
            .filter(|pair| pair.binding().split() == CollaborationLearningSplitV1::Holdout)
            .collect::<Vec<_>>();

        let (status, reason, train_uplift, holdout_uplift) =
            if self.candidate.ordinal > config.candidate_budget {
                frozen(CollaborationLearningFreezeReasonV1::CandidateBudget)
            } else if self.censored_count() > 0 {
                frozen(CollaborationLearningFreezeReasonV1::InvalidInstrumentation)
            } else if pairs.iter().any(|pair| !complete_usage(pair)) {
                frozen(CollaborationLearningFreezeReasonV1::IncompleteResourceReceipts)
            } else if pairs
                .iter()
                .any(|pair| has_safety_or_preservation_failure(pair))
            {
                frozen(CollaborationLearningFreezeReasonV1::SafetyOrPreservation)
            } else if pairs
                .iter()
                .any(|pair| exceeds_resource_regression(pair, config.max_resource_regression_bps))
            {
                frozen(CollaborationLearningFreezeReasonV1::ResourceRegression)
            } else if any_reward_regression(&train)? {
                frozen(CollaborationLearningFreezeReasonV1::NoUplift)
            } else if any_reward_regression(&holdout)? {
                frozen(CollaborationLearningFreezeReasonV1::HoldoutRegression)
            } else if train.len() < usize::from(config.min_train_pairs)
                || holdout.len() < usize::from(config.min_holdout_pairs)
                || !self
                    .frozen_holdout
                    .iter()
                    .all(|digest| self.observations.contains_key(digest))
            {
                (
                    CollaborationLearningAggregateStatusV1::Collecting,
                    None,
                    0,
                    0,
                )
            } else if !position_balanced(&train, config.max_position_imbalance)
                || !position_balanced(&holdout, config.max_position_imbalance)
            {
                frozen(CollaborationLearningFreezeReasonV1::PositionImbalance)
            } else {
                let train_uplift = mean_uplift_bps(&train)?;
                let holdout_uplift = mean_uplift_bps(&holdout)?;
                if train_uplift < i32::from(config.minimum_uplift_bps) {
                    frozen(CollaborationLearningFreezeReasonV1::NoUplift)
                } else if holdout_uplift < i32::from(config.minimum_uplift_bps) {
                    frozen(CollaborationLearningFreezeReasonV1::HoldoutRegression)
                } else {
                    (
                        CollaborationLearningAggregateStatusV1::ReadyForReview,
                        None,
                        train_uplift,
                        holdout_uplift,
                    )
                }
            };
        let aggregate = CollaborationLearningAggregateV1::new(
            status,
            reason,
            self.candidate.digest(),
            &evidence_sha256,
            &config.config_sha256,
            train_uplift,
            holdout_uplift,
        )?;
        if status != CollaborationLearningAggregateStatusV1::Collecting {
            self.sealed = true;
        }
        Ok(aggregate)
    }

    pub fn approve_for_narrow_validation(
        &self,
        aggregate: &CollaborationLearningAggregateV1,
        review: &CollaborationLearningReviewReceiptV1,
    ) -> ContractResult<CollaborationLearningOfflineAdmissionV1> {
        if !self.sealed
            || aggregate.status != CollaborationLearningAggregateStatusV1::ReadyForReview
            || aggregate.candidate_sha256 != self.candidate.candidate_sha256
            || review.candidate_sha256 != self.candidate.candidate_sha256
            || review.evidence_sha256 != aggregate.review_evidence_sha256
            || review.reviewer_identity_sha256 == self.candidate.proposer_identity_sha256
            || !review.approve
        {
            return Err(error("candidate lacks an independent approving review"));
        }
        CollaborationLearningOfflineAdmissionV1::new(aggregate, review)
    }
}

fn changed_axis_exercised(
    candidate: &CollaborationLearningCandidateV1,
    pair: &CollaborationLearningPairV1,
) -> bool {
    let exercise = &pair.workflow.exercise;
    match candidate.changed_axis {
        Some(CollaborationPolicyAxisV1::ContextBudgetBps) => {
            let mut attempts = exercise.lanes.iter().flat_map(|lane| &lane.attempts);
            attempts.next().is_some()
                && attempts.all(|attempt| {
                    attempt.context.budget_bps == candidate.policy.context_budget_bps
                })
        }
        Some(CollaborationPolicyAxisV1::Verification) => {
            !exercise.assignment.plan_required_independent_verifier
        }
        Some(CollaborationPolicyAxisV1::Repair) => {
            if candidate.policy.repair == CollaborationRepairV1::FailFast {
                exercise
                    .lanes
                    .iter()
                    .any(|lane| lane.attempts.len() == 1 && !lane.attempts[0].succeeded)
            } else {
                exercise.lanes.iter().any(|lane| lane.attempts.len() == 2)
            }
        }
        Some(CollaborationPolicyAxisV1::SpecialistInvocation) | None => false,
    }
}

fn complete_usage(pair: &CollaborationLearningPairV1) -> bool {
    [&pair.direct.outcome, &pair.workflow.outcome]
        .iter()
        .all(|outcome| {
            outcome.resources.terminal.segment.completeness()
                == AgentOutcomeUsageCompletenessV1::Complete
                && outcome.resources.terminal.lineage.completeness()
                    == AgentOutcomeUsageCompletenessV1::Complete
        })
}

fn position_balanced(pairs: &[&CollaborationLearningPairV1], max_imbalance: u16) -> bool {
    let direct_first = pairs
        .iter()
        .filter(|pair| pair.binding().arm_order() == CollaborationLearningArmOrderV1::DirectFirst)
        .count();
    direct_first.abs_diff(pairs.len().saturating_sub(direct_first)) <= usize::from(max_imbalance)
}

fn has_safety_or_preservation_failure(pair: &CollaborationLearningPairV1) -> bool {
    [&pair.direct.outcome, &pair.workflow.outcome]
        .iter()
        .any(|outcome| {
            outcome.verifier.safety_violations > 0
                || outcome
                    .postconditions
                    .iter()
                    .any(|receipt| receipt.preservation && !receipt.passed)
        })
}

fn exceeds_resource_regression(pair: &CollaborationLearningPairV1, tolerance_bps: u16) -> bool {
    exceeds_bps(
        pair.workflow
            .outcome
            .resources
            .terminal
            .lineage
            .total_tokens,
        pair.direct.outcome.resources.terminal.lineage.total_tokens,
        tolerance_bps,
    ) || exceeds_bps(
        pair.workflow.outcome.resources.elapsed_ms,
        pair.direct.outcome.resources.elapsed_ms,
        tolerance_bps,
    ) || exceeds_bps(
        pair.workflow
            .outcome
            .resources
            .terminal
            .lineage
            .physical_model_attempts,
        pair.direct
            .outcome
            .resources
            .terminal
            .lineage
            .physical_model_attempts,
        tolerance_bps,
    ) || exceeds_bps(
        u64::try_from(pair.workflow.outcome.resources.logical_model_calls).unwrap_or(u64::MAX),
        u64::try_from(pair.direct.outcome.resources.logical_model_calls).unwrap_or(u64::MAX),
        tolerance_bps,
    ) || exceeds_bps(
        u64::try_from(pair.workflow.outcome.resources.tool_calls).unwrap_or(u64::MAX),
        u64::try_from(pair.direct.outcome.resources.tool_calls).unwrap_or(u64::MAX),
        tolerance_bps,
    )
}

fn exceeds_bps(candidate: u64, baseline: u64, tolerance_bps: u16) -> bool {
    if baseline == 0 {
        return candidate > 0;
    }
    u128::from(candidate) * 10_000 > u128::from(baseline) * (10_000 + u128::from(tolerance_bps))
}

fn any_reward_regression(pairs: &[&CollaborationLearningPairV1]) -> ContractResult<bool> {
    for pair in pairs {
        if pair.workflow_reward_bps()? < pair.direct_reward_bps()? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn mean_uplift_bps(pairs: &[&CollaborationLearningPairV1]) -> ContractResult<i32> {
    let total = pairs.iter().try_fold(0i64, |sum, pair| {
        Ok::<_, CollaborationLearningError>(
            sum + i64::from(pair.workflow_reward_bps()?) - i64::from(pair.direct_reward_bps()?),
        )
    })?;
    Ok(i32::try_from(total / i64::try_from(pairs.len()).unwrap_or(1)).unwrap_or(0))
}

fn frozen(
    reason: CollaborationLearningFreezeReasonV1,
) -> (
    CollaborationLearningAggregateStatusV1,
    Option<CollaborationLearningFreezeReasonV1>,
    i32,
    i32,
) {
    (
        CollaborationLearningAggregateStatusV1::Frozen,
        Some(reason),
        0,
        0,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningAggregateStatusV1 {
    Collecting,
    ReadyForReview,
    Frozen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningFreezeReasonV1 {
    NoUplift,
    InvalidInstrumentation,
    IncompleteResourceReceipts,
    SafetyOrPreservation,
    ResourceRegression,
    HoldoutRegression,
    PositionImbalance,
    CandidateBudget,
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationLearningAggregateV1 {
    status: CollaborationLearningAggregateStatusV1,
    freeze_reason: Option<CollaborationLearningFreezeReasonV1>,
    candidate_sha256: String,
    review_evidence_sha256: String,
}

impl CollaborationLearningAggregateV1 {
    fn new(
        status: CollaborationLearningAggregateStatusV1,
        freeze_reason: Option<CollaborationLearningFreezeReasonV1>,
        candidate_sha256: &str,
        evidence_set_sha256: &str,
        config_sha256: &str,
        train_uplift_bps: i32,
        holdout_uplift_bps: i32,
    ) -> ContractResult<Self> {
        let review_evidence_sha256 = collaboration_learning_sha256(
            EVIDENCE_HASH_DOMAIN,
            &(
                status,
                freeze_reason,
                candidate_sha256,
                evidence_set_sha256,
                config_sha256,
                train_uplift_bps,
                holdout_uplift_bps,
            ),
            "aggregate review evidence",
        )?;
        Ok(Self {
            status,
            freeze_reason,
            candidate_sha256: candidate_sha256.into(),
            review_evidence_sha256,
        })
    }

    pub fn status(&self) -> CollaborationLearningAggregateStatusV1 {
        self.status
    }

    pub fn freeze_reason(&self) -> Option<CollaborationLearningFreezeReasonV1> {
        self.freeze_reason
    }

    pub fn review_evidence_sha256(&self) -> &str {
        &self.review_evidence_sha256
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationLearningReviewReceiptV1 {
    reviewer_identity_sha256: String,
    evidence_sha256: String,
    candidate_sha256: String,
    approve: bool,
    review_sha256: String,
}

impl CollaborationLearningReviewReceiptV1 {
    pub fn new(
        reviewer_identity_sha256: String,
        evidence_sha256: String,
        candidate_sha256: String,
        approve: bool,
    ) -> ContractResult<Self> {
        validate_collaboration_learning_sha256(&reviewer_identity_sha256, "reviewer identity")?;
        validate_collaboration_learning_sha256(&evidence_sha256, "review evidence")?;
        validate_collaboration_learning_sha256(&candidate_sha256, "review candidate")?;
        let mut receipt = Self {
            reviewer_identity_sha256,
            evidence_sha256,
            candidate_sha256,
            approve,
            review_sha256: String::new(),
        };
        receipt.review_sha256 = collaboration_learning_sha256(
            REVIEW_HASH_DOMAIN,
            &(
                COLLABORATION_LEARNING_REVIEW_SCHEMA,
                receipt.reviewer_identity_sha256.as_str(),
                receipt.evidence_sha256.as_str(),
                receipt.candidate_sha256.as_str(),
                receipt.approve,
            ),
            "independent review",
        )?;
        Ok(receipt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningOfflineAdmissionStatusV1 {
    ApprovedForNarrowValidation,
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationLearningOfflineAdmissionV1 {
    status: CollaborationLearningOfflineAdmissionStatusV1,
    candidate_sha256: String,
    evidence_sha256: String,
    review_sha256: String,
    offline_catalog_only: bool,
    production_eligible: bool,
    admission_sha256: String,
}

impl CollaborationLearningOfflineAdmissionV1 {
    fn new(
        aggregate: &CollaborationLearningAggregateV1,
        review: &CollaborationLearningReviewReceiptV1,
    ) -> ContractResult<Self> {
        let mut admission = Self {
            status: CollaborationLearningOfflineAdmissionStatusV1::ApprovedForNarrowValidation,
            candidate_sha256: aggregate.candidate_sha256.clone(),
            evidence_sha256: aggregate.review_evidence_sha256.clone(),
            review_sha256: review.review_sha256.clone(),
            offline_catalog_only: true,
            production_eligible: false,
            admission_sha256: String::new(),
        };
        admission.admission_sha256 = collaboration_learning_sha256(
            ADMISSION_HASH_DOMAIN,
            &(
                COLLABORATION_LEARNING_OFFLINE_ADMISSION_SCHEMA,
                admission.status,
                admission.candidate_sha256.as_str(),
                admission.evidence_sha256.as_str(),
                admission.review_sha256.as_str(),
                admission.offline_catalog_only,
                admission.production_eligible,
            ),
            "offline admission",
        )?;
        Ok(admission)
    }

    pub fn status(&self) -> CollaborationLearningOfflineAdmissionStatusV1 {
        self.status
    }

    pub fn offline_catalog_only(&self) -> bool {
        self.offline_catalog_only
    }

    pub fn production_eligible(&self) -> bool {
        self.production_eligible
    }
}

fn error(message: impl Into<String>) -> CollaborationLearningError {
    CollaborationLearningError::new(format!("collaboration learning {}", message.into()))
}

#[cfg(test)]
#[path = "collaboration_learning_admission_tests.rs"]
mod tests;
