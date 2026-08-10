use crate::collaboration_learning_admission::{
    CollaborationLearningArmOrderV1, CollaborationLearningCensorReasonV1,
    CollaborationLearningComparisonBindingV1, CollaborationLearningSplitV1, MAX_COMPARISONS,
};
use crate::collaboration_learning_policy::{
    collaboration_learning_sha256, validate_collaboration_learning_sha256,
    CollaborationLearningError,
};
#[cfg(feature = "collaboration-learning-offline")]
use serde::Deserialize;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

type ContractResult<T> = Result<T, CollaborationLearningError>;

pub const COLLABORATION_LEARNING_HOLDOUT_RESERVATION_SCHEMA: &str =
    "cindx.agent-collaboration-learning-holdout-reservation.v1";
pub const COLLABORATION_LEARNING_HOLDOUT_CENSOR_SCHEMA: &str =
    "cindx.agent-collaboration-learning-holdout-censor.v1";

const HOLDOUT_RESERVATION_HASH_DOMAIN: &[u8] =
    b"cindx.agent-collaboration-learning-holdout-reservation.v1\0";
const HOLDOUT_RESERVATION_MANIFEST_HASH_DOMAIN: &[u8] =
    b"cindx.agent-collaboration-learning-holdout-reservation-manifest.v1\0";
const HOLDOUT_CENSOR_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-holdout-censor.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(
    feature = "collaboration-learning-offline",
    derive(Deserialize),
    serde(deny_unknown_fields)
)]
pub struct CollaborationLearningHoldoutReservationV1 {
    source_commit_sha256: String,
    suite_sha256: String,
    case_sha256: String,
    prestate_sha256: String,
    provider_sha256: String,
    model_pool_sha256: String,
    budget_sha256: String,
    cohort_sha256: String,
    split: CollaborationLearningSplitV1,
    replicate: u16,
    arm_order: CollaborationLearningArmOrderV1,
    workflow_policy_sha256: String,
    reservation_sha256: String,
}

impl CollaborationLearningHoldoutReservationV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn freeze(
        source_commit_sha256: String,
        suite_sha256: String,
        case_sha256: String,
        prestate_sha256: String,
        provider_sha256: String,
        model_pool_sha256: String,
        budget_sha256: String,
        cohort_sha256: String,
        split: CollaborationLearningSplitV1,
        replicate: u16,
        arm_order: CollaborationLearningArmOrderV1,
        workflow_policy_sha256: String,
    ) -> ContractResult<Self> {
        for (value, label) in [
            (&source_commit_sha256, "reservation source commit"),
            (&suite_sha256, "reservation suite"),
            (&case_sha256, "reservation case"),
            (&prestate_sha256, "reservation prestate"),
            (&provider_sha256, "reservation provider"),
            (&model_pool_sha256, "reservation model pool"),
            (&budget_sha256, "reservation budget"),
            (&cohort_sha256, "reservation cohort"),
            (&workflow_policy_sha256, "reservation workflow policy"),
        ] {
            validate_collaboration_learning_sha256(value, label)?;
        }
        if split != CollaborationLearningSplitV1::Holdout {
            return Err(error("holdout reservation must use the holdout split"));
        }
        if replicate == 0 || usize::from(replicate) > MAX_COMPARISONS {
            return Err(error(
                "holdout reservation replicate is outside its fixed bound",
            ));
        }
        let mut reservation = Self {
            source_commit_sha256,
            suite_sha256,
            case_sha256,
            prestate_sha256,
            provider_sha256,
            model_pool_sha256,
            budget_sha256,
            cohort_sha256,
            split,
            replicate,
            arm_order,
            workflow_policy_sha256,
            reservation_sha256: String::new(),
        };
        reservation.reservation_sha256 = reservation.payload_sha256()?;
        Ok(reservation)
    }

    pub fn from_binding(
        binding: &CollaborationLearningComparisonBindingV1,
        workflow_policy_sha256: String,
    ) -> ContractResult<Self> {
        binding.validate()?;
        let hashes = binding.hashes();
        Self::freeze(
            hashes.source_commit_sha256.clone(),
            hashes.suite_sha256.clone(),
            hashes.case_sha256.clone(),
            hashes.prestate_sha256.clone(),
            hashes.provider_sha256.clone(),
            hashes.model_pool_sha256.clone(),
            hashes.budget_sha256.clone(),
            hashes.cohort_sha256.clone(),
            binding.split(),
            binding.replicate(),
            binding.arm_order(),
            workflow_policy_sha256,
        )
    }

    pub fn freeze_manifest(reservations: &[Self]) -> ContractResult<String> {
        Ok(freeze_holdout_reservations(reservations)?.0)
    }

    pub fn digest(&self) -> &str {
        &self.reservation_sha256
    }

    pub fn split(&self) -> CollaborationLearningSplitV1 {
        self.split
    }

    pub fn replicate(&self) -> u16 {
        self.replicate
    }

    pub fn arm_order(&self) -> CollaborationLearningArmOrderV1 {
        self.arm_order
    }

    pub fn workflow_policy_sha256(&self) -> &str {
        &self.workflow_policy_sha256
    }

    fn payload_sha256(&self) -> ContractResult<String> {
        collaboration_learning_sha256(
            HOLDOUT_RESERVATION_HASH_DOMAIN,
            &(
                COLLABORATION_LEARNING_HOLDOUT_RESERVATION_SCHEMA,
                self.source_commit_sha256.as_str(),
                self.suite_sha256.as_str(),
                self.case_sha256.as_str(),
                self.prestate_sha256.as_str(),
                self.provider_sha256.as_str(),
                self.model_pool_sha256.as_str(),
                self.budget_sha256.as_str(),
                self.cohort_sha256.as_str(),
                self.split,
                self.replicate,
                self.arm_order,
                self.workflow_policy_sha256.as_str(),
            ),
            "holdout reservation",
        )
    }

    pub(crate) fn validate(&self) -> ContractResult<()> {
        let rebuilt = Self::freeze(
            self.source_commit_sha256.clone(),
            self.suite_sha256.clone(),
            self.case_sha256.clone(),
            self.prestate_sha256.clone(),
            self.provider_sha256.clone(),
            self.model_pool_sha256.clone(),
            self.budget_sha256.clone(),
            self.cohort_sha256.clone(),
            self.split,
            self.replicate,
            self.arm_order,
            self.workflow_policy_sha256.clone(),
        )?;
        if self.reservation_sha256 != rebuilt.reservation_sha256 {
            return Err(error("holdout reservation digest is invalid"));
        }
        Ok(())
    }

    pub(crate) fn validate_binding(
        &self,
        binding: &CollaborationLearningComparisonBindingV1,
    ) -> ContractResult<()> {
        self.validate()?;
        binding.validate()?;
        let hashes = binding.hashes();
        if self.source_commit_sha256 != hashes.source_commit_sha256
            || self.suite_sha256 != hashes.suite_sha256
            || self.case_sha256 != hashes.case_sha256
            || self.prestate_sha256 != hashes.prestate_sha256
            || self.provider_sha256 != hashes.provider_sha256
            || self.model_pool_sha256 != hashes.model_pool_sha256
            || self.budget_sha256 != hashes.budget_sha256
            || self.cohort_sha256 != hashes.cohort_sha256
            || self.split != binding.split()
            || self.replicate != binding.replicate()
            || self.arm_order != binding.arm_order()
        {
            return Err(error("observation does not refine its holdout reservation"));
        }
        Ok(())
    }

    pub(crate) fn shares_static_scope(
        &self,
        binding: &CollaborationLearningComparisonBindingV1,
    ) -> bool {
        let hashes = binding.hashes();
        self.source_commit_sha256 == hashes.source_commit_sha256
            && self.suite_sha256 == hashes.suite_sha256
            && self.provider_sha256 == hashes.provider_sha256
            && self.model_pool_sha256 == hashes.model_pool_sha256
            && self.budget_sha256 == hashes.budget_sha256
            && self.cohort_sha256 == hashes.cohort_sha256
    }

    pub(crate) fn matches_locator(
        &self,
        binding: &CollaborationLearningComparisonBindingV1,
    ) -> bool {
        self.case_sha256 == binding.hashes().case_sha256
            && self.replicate == binding.replicate()
            && self.arm_order == binding.arm_order()
    }
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(
    feature = "collaboration-learning-offline",
    derive(Deserialize),
    serde(deny_unknown_fields)
)]
pub struct CollaborationLearningHoldoutCensorReceiptV1 {
    reservation_sha256: String,
    source_sha256: String,
    physical_run_sha256: Vec<String>,
    reason: CollaborationLearningCensorReasonV1,
    receipt_sha256: String,
}

impl CollaborationLearningHoldoutCensorReceiptV1 {
    pub fn for_physical_run(
        reservation: &CollaborationLearningHoldoutReservationV1,
        source_sha256: String,
        physical_run_sha256: String,
        reason: CollaborationLearningCensorReasonV1,
    ) -> ContractResult<Self> {
        Self::for_physical_runs(
            reservation,
            source_sha256,
            vec![physical_run_sha256],
            reason,
        )
    }

    pub fn for_physical_runs(
        reservation: &CollaborationLearningHoldoutReservationV1,
        source_sha256: String,
        physical_run_sha256: Vec<String>,
        reason: CollaborationLearningCensorReasonV1,
    ) -> ContractResult<Self> {
        reservation.validate()?;
        Self::build(
            reservation.digest().to_string(),
            source_sha256,
            physical_run_sha256,
            reason,
        )
    }

    pub fn reservation_sha256(&self) -> &str {
        &self.reservation_sha256
    }

    pub fn physical_run_sha256(&self) -> &[String] {
        &self.physical_run_sha256
    }

    pub fn reason(&self) -> CollaborationLearningCensorReasonV1 {
        self.reason
    }

    pub fn digest(&self) -> &str {
        &self.receipt_sha256
    }

    #[cfg(test)]
    pub(crate) fn corrupt_digest_for_test(&mut self, digest: String) {
        self.receipt_sha256 = digest;
    }

    fn build(
        reservation_sha256: String,
        source_sha256: String,
        mut physical_run_sha256: Vec<String>,
        reason: CollaborationLearningCensorReasonV1,
    ) -> ContractResult<Self> {
        validate_collaboration_learning_sha256(&reservation_sha256, "holdout censor reservation")?;
        validate_collaboration_learning_sha256(&source_sha256, "holdout censor source")?;
        if physical_run_sha256.is_empty() || physical_run_sha256.len() > 2 {
            return Err(error(
                "holdout censor physical run identities are outside their fixed bound",
            ));
        }
        physical_run_sha256.sort();
        for identity in &physical_run_sha256 {
            validate_collaboration_learning_sha256(
                identity,
                "holdout censor physical run identity",
            )?;
        }
        if physical_run_sha256
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(error(
                "holdout censor physical run identities are duplicated",
            ));
        }
        let receipt_sha256 = collaboration_learning_sha256(
            HOLDOUT_CENSOR_HASH_DOMAIN,
            &(
                COLLABORATION_LEARNING_HOLDOUT_CENSOR_SCHEMA,
                reservation_sha256.as_str(),
                source_sha256.as_str(),
                &physical_run_sha256,
                reason,
            ),
            "holdout censor receipt",
        )?;
        Ok(Self {
            reservation_sha256,
            source_sha256,
            physical_run_sha256,
            reason,
            receipt_sha256,
        })
    }

    pub(crate) fn validate(&self) -> ContractResult<()> {
        let rebuilt = Self::build(
            self.reservation_sha256.clone(),
            self.source_sha256.clone(),
            self.physical_run_sha256.clone(),
            self.reason,
        )?;
        if rebuilt.receipt_sha256 != self.receipt_sha256 {
            return Err(error("holdout censor receipt digest is invalid"));
        }
        Ok(())
    }
}

pub(crate) fn freeze_holdout_reservations(
    reservations: &[CollaborationLearningHoldoutReservationV1],
) -> ContractResult<(
    String,
    BTreeMap<String, CollaborationLearningHoldoutReservationV1>,
    BTreeSet<String>,
)> {
    if reservations.is_empty() || reservations.len() > MAX_COMPARISONS {
        return Err(error(
            "holdout reservations must be frozen before candidate proposal",
        ));
    }
    let mut frozen = BTreeMap::new();
    let mut locators = BTreeSet::new();
    let mut cases = BTreeSet::new();
    for reservation in reservations {
        reservation.validate()?;
        let arm_order = match reservation.arm_order {
            CollaborationLearningArmOrderV1::DirectFirst => 0,
            CollaborationLearningArmOrderV1::WorkflowFirst => 1,
        };
        if !locators.insert((
            reservation.case_sha256.clone(),
            reservation.replicate,
            arm_order,
        )) || frozen
            .insert(reservation.digest().to_string(), reservation.clone())
            .is_some()
        {
            return Err(error("holdout reservation is duplicated or ambiguous"));
        }
        cases.insert(reservation.case_sha256.clone());
    }
    let digests = frozen.keys().cloned().collect::<BTreeSet<_>>();
    let manifest = collaboration_learning_sha256(
        HOLDOUT_RESERVATION_MANIFEST_HASH_DOMAIN,
        &digests,
        "holdout reservation manifest",
    )?;
    Ok((manifest, frozen, cases))
}

fn error(message: impl Into<String>) -> CollaborationLearningError {
    CollaborationLearningError::new(format!("collaboration learning {}", message.into()))
}
