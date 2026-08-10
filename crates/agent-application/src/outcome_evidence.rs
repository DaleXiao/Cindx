use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;

pub const EXTERNALLY_VERIFIED_OUTCOME_SCHEMA: &str = "cindx.agent.externally-verified-outcome.v1";

const OUTCOME_HASH_DOMAIN: &[u8] = b"cindx.agent.externally-verified-outcome.v1\0";
const MAX_OUTCOME_JSON_BYTES: usize = 64 * 1024;
const MAX_OUTCOME_POSTCONDITIONS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOutcomeEvidenceError(String);

impl AgentOutcomeEvidenceError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for AgentOutcomeEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AgentOutcomeEvidenceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcomeTerminalStatusV1 {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOutcomeLifecycleBindingV1 {
    pub agent_run_id: String,
    pub steer_epoch: u64,
    pub strategy_receipt_key: String,
    pub strategy_plan_sha256: String,
    pub execution_plan_semantic_sha256: String,
    pub terminal_commit_key: String,
    pub terminal_status: AgentOutcomeTerminalStatusV1,
    pub decision_sequence: u64,
    pub terminal_sequence: u64,
}

impl AgentOutcomeLifecycleBindingV1 {
    pub(crate) fn validate(&self) -> Result<(), AgentOutcomeEvidenceError> {
        if self.agent_run_id.trim().is_empty()
            || self.terminal_sequence <= self.decision_sequence
            || self.strategy_plan_sha256 != self.execution_plan_semantic_sha256
        {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome lifecycle binding is invalid",
            ));
        }
        for (value, label) in [
            (&self.strategy_receipt_key, "strategy receipt key"),
            (&self.strategy_plan_sha256, "strategy plan"),
            (
                &self.execution_plan_semantic_sha256,
                "semantic execution plan",
            ),
            (&self.terminal_commit_key, "terminal commit key"),
        ] {
            validate_sha256(value, label)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOutcomeExposureV1 {
    pub logical_model_calls: usize,
    pub worker_model_calls: usize,
    pub successful_owner_model_calls: usize,
    pub successful_specialist_model_calls: usize,
    pub successful_independent_verifier_model_calls: usize,
    pub successful_conductor_model_calls: usize,
    pub successful_workflow_specialist_model_calls: usize,
    pub successful_workflow_verifier_model_calls: usize,
    pub worker_models: BTreeSet<String>,
    pub successful_workflow_specialist_models: BTreeSet<String>,
    pub successful_workflow_verifier_models: BTreeSet<String>,
    pub direct_anchor_competition_calls: usize,
    pub non_owner_permission_gated_calls: usize,
    pub workflow_planned: bool,
    pub workflow_completed: bool,
}

impl AgentOutcomeExposureV1 {
    pub(crate) fn validate(&self) -> Result<(), AgentOutcomeEvidenceError> {
        let successful_worker_calls = self
            .successful_specialist_model_calls
            .checked_add(self.successful_independent_verifier_model_calls)
            .ok_or_else(|| {
                AgentOutcomeEvidenceError::new(
                    "externally verified outcome treatment exposure overflowed",
                )
            })?;
        let successful_actor_calls = self
            .successful_owner_model_calls
            .checked_add(successful_worker_calls)
            .ok_or_else(|| {
                AgentOutcomeEvidenceError::new(
                    "externally verified outcome treatment exposure overflowed",
                )
            })?;
        let successful_attributed_calls = successful_actor_calls
            .checked_add(self.successful_conductor_model_calls)
            .ok_or_else(|| {
                AgentOutcomeEvidenceError::new(
                    "externally verified outcome treatment exposure overflowed",
                )
            })?;
        let successful_workflow_calls = self
            .successful_workflow_specialist_model_calls
            .checked_add(self.successful_workflow_verifier_model_calls)
            .ok_or_else(|| {
                AgentOutcomeEvidenceError::new(
                    "externally verified outcome treatment exposure overflowed",
                )
            })?;
        if self.direct_anchor_competition_calls > 0
            || self.non_owner_permission_gated_calls > 0
            || (!self.workflow_planned && self.workflow_completed)
            || self.worker_model_calls > self.logical_model_calls
            || successful_attributed_calls > self.logical_model_calls
            || successful_worker_calls > self.worker_model_calls
            || successful_workflow_calls > self.worker_model_calls
            || self.successful_workflow_specialist_model_calls
                > self.successful_specialist_model_calls
            || self.successful_workflow_verifier_model_calls
                > self.successful_independent_verifier_model_calls
            || (!self.workflow_planned && successful_workflow_calls > 0)
            || self.worker_models.len() > self.worker_model_calls
            || (self.worker_model_calls > 0 && self.worker_models.is_empty())
            || self.successful_workflow_specialist_models.len()
                > self.successful_workflow_specialist_model_calls
            || self.successful_workflow_verifier_models.len()
                > self.successful_workflow_verifier_model_calls
            || (self.successful_workflow_specialist_model_calls > 0
                && self.successful_workflow_specialist_models.is_empty())
            || (self.successful_workflow_verifier_model_calls > 0
                && self.successful_workflow_verifier_models.is_empty())
            || !self
                .successful_workflow_specialist_models
                .is_subset(&self.worker_models)
            || !self
                .successful_workflow_verifier_models
                .is_subset(&self.worker_models)
            || self
                .successful_workflow_specialist_models
                .iter()
                .any(|model| self.successful_workflow_verifier_models.contains(model))
        {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome treatment exposure is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcomeUsageCompletenessV1 {
    Complete,
    Partial,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOutcomeUsageV1 {
    pub physical_model_attempts: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub reserved_tokens: u64,
    pub provider_usage_attempts: u64,
    pub partial_usage_attempts: u64,
    pub estimated_usage_attempts: u64,
    pub unknown_usage_attempts: u64,
}

impl AgentOutcomeUsageV1 {
    pub fn completeness(&self) -> AgentOutcomeUsageCompletenessV1 {
        if self.physical_model_attempts == 0
            || self.unknown_usage_attempts > 0
            || self.usage_source_attempts() != Some(self.physical_model_attempts)
        {
            AgentOutcomeUsageCompletenessV1::Missing
        } else if self.partial_usage_attempts > 0 || self.estimated_usage_attempts > 0 {
            AgentOutcomeUsageCompletenessV1::Partial
        } else {
            AgentOutcomeUsageCompletenessV1::Complete
        }
    }

    fn usage_source_attempts(&self) -> Option<u64> {
        self.provider_usage_attempts
            .checked_add(self.partial_usage_attempts)?
            .checked_add(self.estimated_usage_attempts)?
            .checked_add(self.unknown_usage_attempts)
    }

    pub(crate) fn validate(&self) -> Result<(), AgentOutcomeEvidenceError> {
        let accounted_tokens = self
            .prompt_tokens
            .checked_add(self.completion_tokens)
            .ok_or_else(|| {
                AgentOutcomeEvidenceError::new(
                    "externally verified outcome resource token accounting overflowed",
                )
            })?;
        if self.usage_source_attempts() != Some(self.physical_model_attempts)
            || accounted_tokens > self.total_tokens
        {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome resource accounting is inconsistent",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOutcomeTerminalResourcesV1 {
    pub segment: AgentOutcomeUsageV1,
    pub lineage: AgentOutcomeUsageV1,
}

impl AgentOutcomeTerminalResourcesV1 {
    pub(crate) fn validate(&self) -> Result<(), AgentOutcomeEvidenceError> {
        self.segment.validate()?;
        self.lineage.validate()?;
        if self.segment.reserved_tokens != 0
            || self.lineage.reserved_tokens != 0
            || self.segment.completeness() == AgentOutcomeUsageCompletenessV1::Missing
            || self.lineage.completeness() == AgentOutcomeUsageCompletenessV1::Missing
            || self.lineage.physical_model_attempts < self.segment.physical_model_attempts
            || self.lineage.prompt_tokens < self.segment.prompt_tokens
            || self.lineage.completion_tokens < self.segment.completion_tokens
            || self.lineage.total_tokens < self.segment.total_tokens
            || self.lineage.provider_usage_attempts < self.segment.provider_usage_attempts
            || self.lineage.partial_usage_attempts < self.segment.partial_usage_attempts
            || self.lineage.estimated_usage_attempts < self.segment.estimated_usage_attempts
            || self.lineage.unknown_usage_attempts < self.segment.unknown_usage_attempts
        {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome terminal resources are incomplete",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOutcomeResourcesV1 {
    pub budget_sha256: String,
    pub model_receipts_sha256: String,
    pub tool_receipts_sha256: String,
    pub elapsed_ms: u64,
    pub logical_model_calls: usize,
    pub tool_calls: usize,
    pub terminal: AgentOutcomeTerminalResourcesV1,
}

impl AgentOutcomeResourcesV1 {
    pub fn new(
        budget_sha256: String,
        model_receipts_sha256: String,
        tool_receipts_sha256: String,
        elapsed_ms: u64,
        logical_model_calls: usize,
        tool_calls: usize,
        terminal: AgentOutcomeTerminalResourcesV1,
    ) -> Result<Self, AgentOutcomeEvidenceError> {
        let resources = Self {
            budget_sha256,
            model_receipts_sha256,
            tool_receipts_sha256,
            elapsed_ms,
            logical_model_calls,
            tool_calls,
            terminal,
        };
        resources.validate()?;
        Ok(resources)
    }

    fn validate(&self) -> Result<(), AgentOutcomeEvidenceError> {
        validate_sha256(&self.budget_sha256, "budget receipt")?;
        validate_sha256(&self.model_receipts_sha256, "model receipts")?;
        validate_sha256(&self.tool_receipts_sha256, "tool receipts")?;
        self.terminal.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentExternalVerifierV1 {
    pub kind: String,
    pub protocol_sha256: String,
    pub subject_sha256: String,
    pub safety_violations: usize,
}

impl AgentExternalVerifierV1 {
    fn validate(&self) -> Result<(), AgentOutcomeEvidenceError> {
        if self.kind.trim().is_empty() {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome verifier kind is missing",
            ));
        }
        validate_sha256(&self.protocol_sha256, "verifier protocol")?;
        validate_sha256(&self.subject_sha256, "verifier subject")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentExternalPostconditionV1 {
    pub kind: String,
    pub subject_sha256: String,
    pub expected_sha256: String,
    pub observed_sha256: Option<String>,
    pub artifact_sha256: Option<String>,
    pub bytes: Option<u64>,
    pub passed: bool,
    pub preservation: bool,
}

impl AgentExternalPostconditionV1 {
    fn validate(&self) -> Result<(), AgentOutcomeEvidenceError> {
        if self.kind.trim().is_empty() {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome postcondition kind is missing",
            ));
        }
        validate_sha256(&self.subject_sha256, "postcondition subject")?;
        validate_sha256(&self.expected_sha256, "postcondition expectation")?;
        if let Some(value) = &self.observed_sha256 {
            validate_sha256(value, "postcondition observation")?;
        }
        if let Some(value) = &self.artifact_sha256 {
            validate_sha256(value, "postcondition artifact")?;
        }
        if self.artifact_sha256.is_some() != self.bytes.is_some() {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome postcondition artifact size is inconsistent",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentOutcomeDispositionV1 {
    Positive,
    Partial,
    Negative,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternallyVerifiedOutcomeV1 {
    pub schema: String,
    pub lifecycle: AgentOutcomeLifecycleBindingV1,
    pub exposure: AgentOutcomeExposureV1,
    pub verifier: AgentExternalVerifierV1,
    pub postconditions: Vec<AgentExternalPostconditionV1>,
    pub resources: AgentOutcomeResourcesV1,
    pub receipt_sha256: String,
}

impl ExternallyVerifiedOutcomeV1 {
    pub fn new(
        lifecycle: AgentOutcomeLifecycleBindingV1,
        exposure: AgentOutcomeExposureV1,
        verifier: AgentExternalVerifierV1,
        mut postconditions: Vec<AgentExternalPostconditionV1>,
        resources: AgentOutcomeResourcesV1,
    ) -> Result<Self, AgentOutcomeEvidenceError> {
        postconditions.sort_by(|left, right| {
            (
                left.preservation,
                left.kind.as_str(),
                left.subject_sha256.as_str(),
                left.expected_sha256.as_str(),
            )
                .cmp(&(
                    right.preservation,
                    right.kind.as_str(),
                    right.subject_sha256.as_str(),
                    right.expected_sha256.as_str(),
                ))
        });
        let mut outcome = Self {
            schema: EXTERNALLY_VERIFIED_OUTCOME_SCHEMA.to_string(),
            lifecycle,
            exposure,
            verifier,
            postconditions,
            resources,
            receipt_sha256: String::new(),
        };
        outcome.validate_payload()?;
        outcome.receipt_sha256 = outcome.payload_sha256()?;
        Ok(outcome)
    }

    pub fn from_json(encoded: &str) -> Result<Self, AgentOutcomeEvidenceError> {
        if encoded.len() > MAX_OUTCOME_JSON_BYTES {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome JSON exceeds its size bound",
            ));
        }
        let outcome = serde_json::from_str::<Self>(encoded).map_err(|error| {
            AgentOutcomeEvidenceError::new(format!(
                "externally verified outcome JSON is invalid: {error}"
            ))
        })?;
        outcome.validate()?;
        Ok(outcome)
    }

    pub fn to_json(&self) -> Result<String, AgentOutcomeEvidenceError> {
        self.validate()?;
        let encoded = serde_json::to_string(self).map_err(|error| {
            AgentOutcomeEvidenceError::new(format!(
                "externally verified outcome JSON encoding failed: {error}"
            ))
        })?;
        if encoded.len() > MAX_OUTCOME_JSON_BYTES {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome JSON exceeds its size bound",
            ));
        }
        Ok(encoded)
    }

    pub fn reward_fraction(&self) -> Result<(usize, usize), AgentOutcomeEvidenceError> {
        self.validate()?;
        let behavior = self
            .postconditions
            .iter()
            .filter(|receipt| !receipt.preservation)
            .collect::<Vec<_>>();
        let total = behavior.len();
        let hard_failure = self.lifecycle.terminal_status
            != AgentOutcomeTerminalStatusV1::Completed
            || self.verifier.safety_violations > 0
            || self
                .postconditions
                .iter()
                .any(|receipt| receipt.preservation && !receipt.passed);
        let passed = if hard_failure {
            0
        } else {
            behavior.iter().filter(|receipt| receipt.passed).count()
        };
        Ok((passed, total))
    }

    pub fn reward_bps(&self) -> Result<u16, AgentOutcomeEvidenceError> {
        let (passed, total) = self.reward_fraction()?;
        Ok(u16::try_from(passed.saturating_mul(10_000) / total.max(1)).unwrap_or(10_000))
    }

    pub fn disposition(&self) -> Result<AgentOutcomeDispositionV1, AgentOutcomeEvidenceError> {
        Ok(match self.reward_bps()? {
            10_000 => AgentOutcomeDispositionV1::Positive,
            0 => AgentOutcomeDispositionV1::Negative,
            _ => AgentOutcomeDispositionV1::Partial,
        })
    }

    pub fn validate(&self) -> Result<(), AgentOutcomeEvidenceError> {
        self.validate_payload()?;
        let expected = self.payload_sha256()?;
        if self.receipt_sha256 != expected {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome digest is invalid",
            ));
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), AgentOutcomeEvidenceError> {
        if self.schema != EXTERNALLY_VERIFIED_OUTCOME_SCHEMA {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome schema is unsupported",
            ));
        }
        self.lifecycle.validate()?;
        if self.lifecycle.terminal_status == AgentOutcomeTerminalStatusV1::Cancelled {
            return Err(AgentOutcomeEvidenceError::new(
                "cancelled runs are censored from externally verified outcomes",
            ));
        }
        self.verifier.validate()?;
        self.resources.validate()?;
        self.exposure.validate()?;
        if self.resources.logical_model_calls != self.exposure.logical_model_calls {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome exposure and resource call counts disagree",
            ));
        }
        if self.exposure.non_owner_permission_gated_calls > 0 {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome exposed permission authority outside Owner",
            ));
        }
        if self.postconditions.is_empty() || self.postconditions.len() > MAX_OUTCOME_POSTCONDITIONS
        {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome postcondition count is invalid",
            ));
        }
        let mut identities = BTreeSet::new();
        let mut behavior_checks = 0usize;
        for receipt in &self.postconditions {
            receipt.validate()?;
            if !identities.insert((
                receipt.preservation,
                receipt.kind.as_str(),
                receipt.subject_sha256.as_str(),
                receipt.expected_sha256.as_str(),
            )) {
                return Err(AgentOutcomeEvidenceError::new(
                    "externally verified outcome has duplicate postconditions",
                ));
            }
            behavior_checks += usize::from(!receipt.preservation);
        }
        if behavior_checks == 0 {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome has no behavior postcondition",
            ));
        }
        Ok(())
    }

    fn payload_sha256(&self) -> Result<String, AgentOutcomeEvidenceError> {
        let payload = serde_json::to_vec(&(
            self.schema.as_str(),
            &self.lifecycle,
            &self.exposure,
            &self.verifier,
            &self.postconditions,
            &self.resources,
        ))
        .map_err(|error| {
            AgentOutcomeEvidenceError::new(format!(
                "externally verified outcome digest encoding failed: {error}"
            ))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(OUTCOME_HASH_DOMAIN);
        hasher.update(payload);
        Ok(hex_digest(hasher.finalize().as_slice()))
    }
}

pub(crate) fn validate_sha256(value: &str, label: &str) -> Result<(), AgentOutcomeEvidenceError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AgentOutcomeEvidenceError::new(format!(
            "externally verified outcome {label} is not a SHA-256 digest"
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
