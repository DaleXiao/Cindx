use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

pub const COLLABORATION_LEARNING_POLICY_SCHEMA: &str =
    "cindx.agent-collaboration-learning-policy.v1";
pub const COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS: u16 = 5_000;
pub const COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS: u16 = 7_500;

const COLLABORATION_LEARNING_POLICY_HASH_DOMAIN: &[u8] =
    b"cindx.agent-collaboration-learning-policy.v1\0";
const MAX_COLLABORATION_LEARNING_POLICY_JSON_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationLearningError(String);

impl CollaborationLearningError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CollaborationLearningError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CollaborationLearningError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationGoal2LimitsV1 {
    pub owner_final_delivery: bool,
    pub owner_exclusive_side_effects: bool,
    pub max_specialists: u8,
    pub max_distinct_verifiers: u8,
    pub serial_execution: bool,
    pub workers_read_only: bool,
}

impl CollaborationGoal2LimitsV1 {
    fn fixed() -> Self {
        Self {
            owner_final_delivery: true,
            owner_exclusive_side_effects: true,
            max_specialists: 1,
            max_distinct_verifiers: 1,
            serial_execution: true,
            workers_read_only: true,
        }
    }

    fn validate(&self) -> Result<(), CollaborationLearningError> {
        if self != &Self::fixed() {
            return Err(CollaborationLearningError::new(
                "collaboration learning policy violates the fixed Goal 2 execution limits",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationSpecialistInvocationV1 {
    DirectOwnerOnly,
    OneReadOnlySpecialist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationVerificationV1 {
    PlanRequiredOnly,
    AlwaysIndependent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationRepairV1 {
    FailFast,
    SameModelOnce,
    AlternateModelOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationStopV1 {
    DerivedFromRequiredLanesAndRepair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationPolicyAxisV1 {
    SpecialistInvocation,
    ContextBudgetBps,
    Verification,
    Repair,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningPolicyV1 {
    pub schema: String,
    pub parent_policy_sha256: Option<String>,
    pub limits: CollaborationGoal2LimitsV1,
    pub specialist_invocation: CollaborationSpecialistInvocationV1,
    pub context_budget_bps: u16,
    pub verification: CollaborationVerificationV1,
    pub repair: CollaborationRepairV1,
    pub stop: CollaborationStopV1,
    pub policy_sha256: String,
}

impl CollaborationLearningPolicyV1 {
    pub fn seed(
        specialist_invocation: CollaborationSpecialistInvocationV1,
        context_budget_bps: u16,
        verification: CollaborationVerificationV1,
        repair: CollaborationRepairV1,
    ) -> Result<Self, CollaborationLearningError> {
        Self::build(
            None,
            specialist_invocation,
            context_budget_bps,
            verification,
            repair,
        )
    }

    pub fn candidate(
        parent: &Self,
        specialist_invocation: CollaborationSpecialistInvocationV1,
        context_budget_bps: u16,
        verification: CollaborationVerificationV1,
        repair: CollaborationRepairV1,
    ) -> Result<Self, CollaborationLearningError> {
        parent.validate()?;
        let candidate = Self::build(
            Some(parent.policy_sha256.clone()),
            specialist_invocation,
            context_budget_bps,
            verification,
            repair,
        )?;
        candidate.changed_axis_against(parent)?;
        Ok(candidate)
    }

    pub fn from_json(encoded: &str) -> Result<Self, CollaborationLearningError> {
        if encoded.len() > MAX_COLLABORATION_LEARNING_POLICY_JSON_BYTES {
            return Err(CollaborationLearningError::new(
                "collaboration learning policy JSON exceeds its size bound",
            ));
        }
        let policy = serde_json::from_str::<Self>(encoded).map_err(|error| {
            CollaborationLearningError::new(format!(
                "collaboration learning policy JSON is invalid: {error}"
            ))
        })?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn to_json(&self) -> Result<String, CollaborationLearningError> {
        self.validate()?;
        let encoded = serde_json::to_string(self).map_err(|error| {
            CollaborationLearningError::new(format!(
                "collaboration learning policy JSON encoding failed: {error}"
            ))
        })?;
        if encoded.len() > MAX_COLLABORATION_LEARNING_POLICY_JSON_BYTES {
            return Err(CollaborationLearningError::new(
                "collaboration learning policy JSON exceeds its size bound",
            ));
        }
        Ok(encoded)
    }

    pub fn validate(&self) -> Result<(), CollaborationLearningError> {
        self.validate_payload()?;
        validate_collaboration_learning_sha256(&self.policy_sha256, "policy")?;
        if self.policy_sha256 != self.payload_sha256()? {
            return Err(CollaborationLearningError::new(
                "collaboration learning policy digest is invalid",
            ));
        }
        Ok(())
    }

    pub fn changed_axis_against(
        &self,
        parent: &Self,
    ) -> Result<CollaborationPolicyAxisV1, CollaborationLearningError> {
        self.validate()?;
        parent.validate()?;
        if self.specialist_invocation != CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
            || parent.specialist_invocation
                != CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
            || self.parent_policy_sha256.as_deref() != Some(parent.policy_sha256.as_str())
        {
            return Err(CollaborationLearningError::new(
                "collaboration learning lineage is not bound to its workflow parent",
            ));
        }

        let mut changed = [
            (
                self.specialist_invocation != parent.specialist_invocation,
                CollaborationPolicyAxisV1::SpecialistInvocation,
            ),
            (
                self.context_budget_bps != parent.context_budget_bps,
                CollaborationPolicyAxisV1::ContextBudgetBps,
            ),
            (
                self.verification != parent.verification,
                CollaborationPolicyAxisV1::Verification,
            ),
            (
                self.repair != parent.repair,
                CollaborationPolicyAxisV1::Repair,
            ),
        ]
        .into_iter()
        .filter_map(|(is_changed, axis)| is_changed.then_some(axis));
        let Some(axis) = changed.next() else {
            return Err(CollaborationLearningError::new(
                "collaboration learning candidate must change exactly one policy axis",
            ));
        };
        if changed.next().is_some() {
            return Err(CollaborationLearningError::new(
                "collaboration learning candidate must change exactly one policy axis",
            ));
        }
        Ok(axis)
    }

    fn build(
        parent_policy_sha256: Option<String>,
        specialist_invocation: CollaborationSpecialistInvocationV1,
        context_budget_bps: u16,
        verification: CollaborationVerificationV1,
        repair: CollaborationRepairV1,
    ) -> Result<Self, CollaborationLearningError> {
        let mut policy = Self {
            schema: COLLABORATION_LEARNING_POLICY_SCHEMA.to_string(),
            parent_policy_sha256,
            limits: CollaborationGoal2LimitsV1::fixed(),
            specialist_invocation,
            context_budget_bps,
            verification,
            repair,
            stop: CollaborationStopV1::DerivedFromRequiredLanesAndRepair,
            policy_sha256: String::new(),
        };
        policy.validate_payload()?;
        policy.policy_sha256 = policy.payload_sha256()?;
        policy.validate()?;
        Ok(policy)
    }

    fn validate_payload(&self) -> Result<(), CollaborationLearningError> {
        if self.schema != COLLABORATION_LEARNING_POLICY_SCHEMA {
            return Err(CollaborationLearningError::new(
                "collaboration learning policy schema is unsupported",
            ));
        }
        self.limits.validate()?;
        if let Some(parent_policy_sha256) = &self.parent_policy_sha256 {
            validate_collaboration_learning_sha256(parent_policy_sha256, "parent policy")?;
            if self.specialist_invocation
                != CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
            {
                return Err(CollaborationLearningError::new(
                    "collaboration learning candidates must remain on the workflow topology",
                ));
            }
        }
        match self.specialist_invocation {
            CollaborationSpecialistInvocationV1::DirectOwnerOnly => {
                if self.context_budget_bps != 0
                    || self.verification != CollaborationVerificationV1::PlanRequiredOnly
                    || self.repair != CollaborationRepairV1::FailFast
                {
                    return Err(CollaborationLearningError::new(
                        "direct collaboration learning policy must remain Owner-only",
                    ));
                }
            }
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist => {
                if !matches!(
                    self.context_budget_bps,
                    COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS
                        | COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS
                ) {
                    return Err(CollaborationLearningError::new(
                        "workflow collaboration context budget is outside the fixed allowlist",
                    ));
                }
            }
        }
        Ok(())
    }

    fn payload_sha256(&self) -> Result<String, CollaborationLearningError> {
        collaboration_learning_sha256(
            COLLABORATION_LEARNING_POLICY_HASH_DOMAIN,
            &(
                self.schema.as_str(),
                self.parent_policy_sha256.as_deref(),
                &self.limits,
                self.specialist_invocation,
                self.context_budget_bps,
                self.verification,
                self.repair,
                self.stop,
            ),
            "policy",
        )
    }
}

pub(crate) fn collaboration_learning_sha256<T: Serialize>(
    domain: &[u8],
    payload: &T,
    label: &str,
) -> Result<String, CollaborationLearningError> {
    let encoded = serde_json::to_vec(payload).map_err(|error| {
        CollaborationLearningError::new(format!(
            "collaboration learning {label} digest encoding failed: {error}"
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(encoded);
    Ok(hex_digest(hasher.finalize().as_slice()))
}

pub(crate) fn validate_collaboration_learning_sha256(
    value: &str,
    label: &str,
) -> Result<(), CollaborationLearningError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CollaborationLearningError::new(format!(
            "collaboration learning {label} is not a SHA-256 digest"
        )));
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow_seed() -> CollaborationLearningPolicyV1 {
        CollaborationLearningPolicyV1::seed(
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
            CollaborationVerificationV1::PlanRequiredOnly,
            CollaborationRepairV1::FailFast,
        )
        .unwrap()
    }

    #[test]
    fn agent_collaboration_learning_contract_policy_is_canonical_and_tamper_evident() {
        println!("{COLLABORATION_LEARNING_POLICY_SCHEMA}");
        let policy = workflow_seed();
        let encoded = policy.to_json().unwrap();
        let decoded = CollaborationLearningPolicyV1::from_json(&encoded).unwrap();
        assert_eq!(decoded, policy);
        assert_eq!(decoded.to_json().unwrap(), encoded);

        let unknown = encoded.replacen('{', "{\"unknown\":true,", 1);
        assert!(CollaborationLearningPolicyV1::from_json(&unknown).is_err());

        let tampered =
            encoded.replace("\"context_budget_bps\":5000", "\"context_budget_bps\":7500");
        assert!(CollaborationLearningPolicyV1::from_json(&tampered).is_err());
    }

    #[test]
    fn agent_collaboration_learning_contract_policy_enforces_goal2_and_axis_values() {
        let mut direct = CollaborationLearningPolicyV1::seed(
            CollaborationSpecialistInvocationV1::DirectOwnerOnly,
            0,
            CollaborationVerificationV1::PlanRequiredOnly,
            CollaborationRepairV1::FailFast,
        )
        .unwrap();
        direct.context_budget_bps = COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS;
        assert!(direct.validate().is_err());

        let mut workflow = workflow_seed();
        workflow.context_budget_bps = 6_000;
        assert!(workflow.validate().is_err());

        let mut unsafe_limits = workflow_seed();
        unsafe_limits.limits.max_specialists = 2;
        assert!(unsafe_limits.validate().is_err());
    }

    #[test]
    fn agent_collaboration_learning_contract_policy_requires_single_axis_workflow_lineage() {
        let parent = workflow_seed();
        let candidate = CollaborationLearningPolicyV1::candidate(
            &parent,
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
            CollaborationVerificationV1::PlanRequiredOnly,
            CollaborationRepairV1::FailFast,
        )
        .unwrap();
        assert_eq!(
            candidate.changed_axis_against(&parent).unwrap(),
            CollaborationPolicyAxisV1::ContextBudgetBps
        );

        assert!(CollaborationLearningPolicyV1::candidate(
            &parent,
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
            CollaborationVerificationV1::AlwaysIndependent,
            CollaborationRepairV1::FailFast,
        )
        .is_err());

        let direct_parent = CollaborationLearningPolicyV1::seed(
            CollaborationSpecialistInvocationV1::DirectOwnerOnly,
            0,
            CollaborationVerificationV1::PlanRequiredOnly,
            CollaborationRepairV1::FailFast,
        )
        .unwrap();
        assert!(CollaborationLearningPolicyV1::candidate(
            &direct_parent,
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
            CollaborationVerificationV1::PlanRequiredOnly,
            CollaborationRepairV1::FailFast,
        )
        .is_err());
    }
}
