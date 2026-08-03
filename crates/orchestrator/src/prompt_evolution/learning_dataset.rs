use super::PromptEvaluationSplit;
use crate::{
    AgentPolicy, LearningAttribution, LearningDisposition, LearningEvidenceV1, LearningTermination,
    LearningUsageCompleteness,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const PROMPT_LEARNING_ELIGIBILITY_SCHEMA_V1: &str = "cindx.prompt-learning-eligibility.v1";
pub const PROMPT_LEARNING_REDACTION_SCHEMA_V1: &str = "cindx.redaction.v1";
pub const PROMPT_DATASET_IDENTITY_SCHEMA_V1: &str = "cindx.prompt-dataset-identity.v1";
pub const PROMPT_EXECUTION_CONTEXT_SCHEMA_V1: &str = "cindx.prompt-execution-context.v1";
pub const PROMPT_LEARNING_COHORT_SCHEMA_V1: &str = "cindx.prompt-learning-cohort.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptLearningPurpose {
    ObjectiveReplay,
    AutoTeacher,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptLearningEligibility {
    Positive,
    Negative,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptLearningRejection {
    None,
    MissingEvidence,
    UntrustedEvidence,
    NonterminalRun,
    StaleSteerEpoch,
    MissingUsage,
    PromptContractFailed,
    PermissionDenied,
    SafetyViolation,
    RedactionUnverified,
    ResidualSensitiveData,
    MissingIdentity,
    EmptyObjective,
    TeacherNotPositive,
    TeacherNotWorkflow,
    TeacherUsageIncomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptLearningEligibilityReceiptV1 {
    pub schema: String,
    pub purpose: PromptLearningPurpose,
    pub eligibility: PromptLearningEligibility,
    pub rejection: PromptLearningRejection,
    pub project_sha256: String,
    pub source_run_sha256: String,
    pub task_family_sha256: String,
    pub objective_sha256: String,
    pub learning_evidence_sha256: String,
    pub steer_epoch: u64,
    pub redaction_schema: String,
}

pub struct PromptLearningQualificationInput<'a> {
    pub purpose: PromptLearningPurpose,
    pub project_id: &'a str,
    pub source_run_id: &'a str,
    pub task_family: &'a str,
    pub redacted_objective: &'a str,
    pub redaction_schema: &'a str,
    pub redaction_verified: bool,
    pub residual_sensitive_data: bool,
    pub steer_epoch: u64,
    pub prompt_contract_passed: bool,
    pub permission_denied: bool,
    pub safety_violations: u64,
    pub evidence: Option<&'a LearningEvidenceV1>,
}

impl PromptLearningEligibilityReceiptV1 {
    pub fn qualify(input: PromptLearningQualificationInput<'_>) -> Self {
        let evidence_sha256 = input
            .evidence
            .and_then(|evidence| serde_json::to_vec(evidence).ok())
            .map(|encoded| learning_sha256_hex(&encoded))
            .unwrap_or_else(|| learning_sha256_hex(b"missing"));
        let mut receipt = Self {
            schema: PROMPT_LEARNING_ELIGIBILITY_SCHEMA_V1.to_string(),
            purpose: input.purpose,
            eligibility: PromptLearningEligibility::Rejected,
            rejection: PromptLearningRejection::None,
            project_sha256: learning_sha256_hex(input.project_id.trim().as_bytes()),
            source_run_sha256: learning_sha256_hex(input.source_run_id.trim().as_bytes()),
            task_family_sha256: learning_sha256_hex(input.task_family.trim().as_bytes()),
            objective_sha256: learning_sha256_hex(input.redacted_objective.trim().as_bytes()),
            learning_evidence_sha256: evidence_sha256,
            steer_epoch: input.steer_epoch,
            redaction_schema: input.redaction_schema.to_string(),
        };
        receipt.rejection = qualification_rejection(&input);
        if receipt.rejection == PromptLearningRejection::None {
            receipt.eligibility = match input.evidence.map(|evidence| evidence.disposition) {
                Some(LearningDisposition::Positive) => PromptLearningEligibility::Positive,
                Some(LearningDisposition::Negative) => PromptLearningEligibility::Negative,
                _ => PromptLearningEligibility::Rejected,
            };
        }
        receipt
    }

    pub fn is_eligible(&self) -> bool {
        self.rejection == PromptLearningRejection::None
            && matches!(
                self.eligibility,
                PromptLearningEligibility::Positive | PromptLearningEligibility::Negative
            )
            && self.validate().is_ok()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_LEARNING_ELIGIBILITY_SCHEMA_V1
            || self.redaction_schema != PROMPT_LEARNING_REDACTION_SCHEMA_V1
            || !is_sha256(&self.project_sha256)
            || !is_sha256(&self.source_run_sha256)
            || !is_sha256(&self.task_family_sha256)
            || !is_sha256(&self.objective_sha256)
            || !is_sha256(&self.learning_evidence_sha256)
        {
            return Err("prompt learning eligibility receipt is malformed".to_string());
        }
        let eligible = matches!(
            self.eligibility,
            PromptLearningEligibility::Positive | PromptLearningEligibility::Negative
        );
        if eligible != (self.rejection == PromptLearningRejection::None) {
            return Err("prompt learning eligibility disposition is inconsistent".to_string());
        }
        if self.purpose == PromptLearningPurpose::AutoTeacher
            && self.eligibility != PromptLearningEligibility::Positive
        {
            return Err("Auto teacher receipt is not positive".to_string());
        }
        Ok(())
    }
}

fn qualification_rejection(
    input: &PromptLearningQualificationInput<'_>,
) -> PromptLearningRejection {
    if input.project_id.trim().is_empty()
        || input.source_run_id.trim().is_empty()
        || input.task_family.trim().is_empty()
    {
        return PromptLearningRejection::MissingIdentity;
    }
    if input.redacted_objective.trim().is_empty() {
        return PromptLearningRejection::EmptyObjective;
    }
    if !input.redaction_verified || input.redaction_schema != PROMPT_LEARNING_REDACTION_SCHEMA_V1 {
        return PromptLearningRejection::RedactionUnverified;
    }
    if input.residual_sensitive_data {
        return PromptLearningRejection::ResidualSensitiveData;
    }
    if input.permission_denied {
        return PromptLearningRejection::PermissionDenied;
    }
    if input.safety_violations > 0 {
        return PromptLearningRejection::SafetyViolation;
    }
    if !input.prompt_contract_passed {
        return PromptLearningRejection::PromptContractFailed;
    }
    let Some(evidence) = input.evidence else {
        return PromptLearningRejection::MissingEvidence;
    };
    if !evidence.is_learnable() {
        return PromptLearningRejection::UntrustedEvidence;
    }
    if evidence.termination != LearningTermination::Completed {
        return PromptLearningRejection::NonterminalRun;
    }
    if evidence.steer_epoch != Some(input.steer_epoch) {
        return PromptLearningRejection::StaleSteerEpoch;
    }
    if evidence.usage_completeness == LearningUsageCompleteness::Missing {
        return PromptLearningRejection::MissingUsage;
    }
    if input.purpose == PromptLearningPurpose::AutoTeacher {
        if evidence.disposition != LearningDisposition::Positive {
            return PromptLearningRejection::TeacherNotPositive;
        }
        if evidence.attribution != LearningAttribution::Workflow
            || evidence.independent_quality_source.is_none()
        {
            return PromptLearningRejection::TeacherNotWorkflow;
        }
        if evidence.usage_completeness != LearningUsageCompleteness::Complete {
            return PromptLearningRejection::TeacherUsageIncomplete;
        }
    }
    PromptLearningRejection::None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDatasetCaseIdentityV1 {
    pub case_id: String,
    pub objective_sha256: String,
    pub task_family_sha256: String,
    pub split: PromptEvaluationSplit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDatasetIdentityV1 {
    pub schema: String,
    pub scope_sha256: String,
    pub generation: u32,
    pub cases: Vec<PromptDatasetCaseIdentityV1>,
    pub train_sha256: String,
    pub holdout_sha256: String,
    pub dataset_sha256: String,
}

impl PromptDatasetIdentityV1 {
    pub fn new(
        scope: &str,
        generation: u32,
        mut cases: Vec<PromptDatasetCaseIdentityV1>,
    ) -> Result<Self, String> {
        cases.sort_by(|left, right| left.case_id.cmp(&right.case_id));
        let scope_sha256 = learning_sha256_hex(scope.trim().as_bytes());
        let train_sha256 = split_digest(&cases, PromptEvaluationSplit::Train)?;
        let holdout_sha256 = split_digest(&cases, PromptEvaluationSplit::Holdout)?;
        let dataset_sha256 = dataset_digest(&scope_sha256, &cases, &train_sha256, &holdout_sha256)?;
        let identity = Self {
            schema: PROMPT_DATASET_IDENTITY_SCHEMA_V1.to_string(),
            scope_sha256,
            generation,
            cases,
            train_sha256,
            holdout_sha256,
            dataset_sha256,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_DATASET_IDENTITY_SCHEMA_V1
            || !is_sha256(&self.scope_sha256)
            || !is_sha256(&self.train_sha256)
            || !is_sha256(&self.holdout_sha256)
            || !is_sha256(&self.dataset_sha256)
        {
            return Err("prompt dataset identity is malformed".to_string());
        }
        let mut case_ids = BTreeSet::new();
        let mut objective_digests = BTreeSet::new();
        let mut previous = None::<&str>;
        for case in &self.cases {
            if case.case_id.trim().is_empty()
                || case.case_id.len() > 256
                || !is_sha256(&case.objective_sha256)
                || !is_sha256(&case.task_family_sha256)
                || !case_ids.insert(case.case_id.as_str())
                || !objective_digests.insert(case.objective_sha256.as_str())
                || previous.is_some_and(|previous| previous >= case.case_id.as_str())
            {
                return Err("prompt dataset cases are not uniquely canonical".to_string());
            }
            previous = Some(case.case_id.as_str());
        }
        if split_digest(&self.cases, PromptEvaluationSplit::Train)? != self.train_sha256
            || split_digest(&self.cases, PromptEvaluationSplit::Holdout)? != self.holdout_sha256
            || dataset_digest(
                &self.scope_sha256,
                &self.cases,
                &self.train_sha256,
                &self.holdout_sha256,
            )? != self.dataset_sha256
        {
            return Err("prompt dataset identity digest does not match its manifest".to_string());
        }
        Ok(())
    }

    pub fn case(&self, case_id: &str) -> Option<&PromptDatasetCaseIdentityV1> {
        self.cases.iter().find(|case| case.case_id == case_id)
    }
}

fn split_digest(
    cases: &[PromptDatasetCaseIdentityV1],
    split: PromptEvaluationSplit,
) -> Result<String, String> {
    let selected = cases
        .iter()
        .filter(|case| case.split == split)
        .collect::<Vec<_>>();
    serde_json::to_vec(&selected)
        .map(|encoded| learning_sha256_hex(&encoded))
        .map_err(|error| format!("prompt dataset split serialization failed: {error}"))
}

fn dataset_digest(
    scope_sha256: &str,
    cases: &[PromptDatasetCaseIdentityV1],
    train_sha256: &str,
    holdout_sha256: &str,
) -> Result<String, String> {
    serde_json::to_vec(&(scope_sha256, cases, train_sha256, holdout_sha256))
        .map(|encoded| learning_sha256_hex(&encoded))
        .map_err(|error| format!("prompt dataset serialization failed: {error}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptExecutionContextV1 {
    pub schema: String,
    pub provider_sha256: String,
    pub model_pool_sha256: String,
    pub harness_sha256: String,
    pub system_prompt_sha256: String,
    pub policy: AgentPolicy,
    pub policy_sha256: String,
    pub budget_sha256: String,
    pub tool_contract_sha256: String,
    pub source_revision_sha256: String,
    pub workspace_revision_sha256: String,
}

impl PromptExecutionContextV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_EXECUTION_CONTEXT_SCHEMA_V1
            || [
                &self.provider_sha256,
                &self.model_pool_sha256,
                &self.harness_sha256,
                &self.system_prompt_sha256,
                &self.policy_sha256,
                &self.budget_sha256,
                &self.tool_contract_sha256,
                &self.source_revision_sha256,
                &self.workspace_revision_sha256,
            ]
            .into_iter()
            .any(|digest| !is_sha256(digest))
        {
            return Err("prompt execution context is malformed".to_string());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_vec(self)
            .map(|encoded| learning_sha256_hex(&encoded))
            .map_err(|error| format!("prompt execution context serialization failed: {error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptLearningCohortV1 {
    pub schema: String,
    pub dataset: PromptDatasetIdentityV1,
    pub execution: PromptExecutionContextV1,
    pub cohort_sha256: String,
}

impl PromptLearningCohortV1 {
    pub fn new(
        dataset: PromptDatasetIdentityV1,
        execution: PromptExecutionContextV1,
    ) -> Result<Self, String> {
        dataset.validate()?;
        execution.validate()?;
        let cohort_sha256 = cohort_digest(&dataset, &execution)?;
        Ok(Self {
            schema: PROMPT_LEARNING_COHORT_SCHEMA_V1.to_string(),
            dataset,
            execution,
            cohort_sha256,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        self.dataset.validate()?;
        self.execution.validate()?;
        if self.schema != PROMPT_LEARNING_COHORT_SCHEMA_V1
            || self.cohort_sha256 != cohort_digest(&self.dataset, &self.execution)?
        {
            return Err("prompt learning cohort digest is invalid".to_string());
        }
        Ok(())
    }
}

fn cohort_digest(
    dataset: &PromptDatasetIdentityV1,
    execution: &PromptExecutionContextV1,
) -> Result<String, String> {
    serde_json::to_vec(&(dataset, execution))
        .map(|encoded| learning_sha256_hex(&encoded))
        .map_err(|error| format!("prompt learning cohort serialization failed: {error}"))
}

fn learning_sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub(super) fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IndependentQualitySource, LearningVerification};

    fn evidence(disposition: LearningDisposition) -> LearningEvidenceV1 {
        LearningEvidenceV1 {
            disposition,
            verification: match disposition {
                LearningDisposition::Positive => LearningVerification::Passed,
                LearningDisposition::Negative => LearningVerification::Failed,
                LearningDisposition::Censored => LearningVerification::Unknown,
            },
            termination: LearningTermination::Completed,
            attribution: LearningAttribution::Workflow,
            usage_completeness: LearningUsageCompleteness::Complete,
            steer_epoch: Some(2),
            budget_fingerprint: Some("b".repeat(64)),
            independent_quality_source: Some(IndependentQualitySource::AnytimeSelector),
            quality_bps: Some(8_000),
            ..LearningEvidenceV1::default()
        }
    }

    fn qualification<'a>(
        evidence: Option<&'a LearningEvidenceV1>,
    ) -> PromptLearningQualificationInput<'a> {
        PromptLearningQualificationInput {
            purpose: PromptLearningPurpose::ObjectiveReplay,
            project_id: "project",
            source_run_id: "run",
            task_family: "coding",
            redacted_objective: "Inspect the implementation",
            redaction_schema: PROMPT_LEARNING_REDACTION_SCHEMA_V1,
            redaction_verified: true,
            residual_sensitive_data: false,
            steer_epoch: 2,
            prompt_contract_passed: true,
            permission_denied: false,
            safety_violations: 0,
            evidence,
        }
    }

    #[test]
    fn eligibility_fails_closed_for_untrusted_or_sensitive_runs() {
        let positive = evidence(LearningDisposition::Positive);
        assert!(
            PromptLearningEligibilityReceiptV1::qualify(qualification(Some(&positive)))
                .is_eligible()
        );
        assert_eq!(
            PromptLearningEligibilityReceiptV1::qualify(qualification(None)).rejection,
            PromptLearningRejection::MissingEvidence
        );
        let mut sensitive = qualification(Some(&positive));
        sensitive.residual_sensitive_data = true;
        assert_eq!(
            PromptLearningEligibilityReceiptV1::qualify(sensitive).rejection,
            PromptLearningRejection::ResidualSensitiveData
        );
        let mut denied = qualification(Some(&positive));
        denied.permission_denied = true;
        assert_eq!(
            PromptLearningEligibilityReceiptV1::qualify(denied).rejection,
            PromptLearningRejection::PermissionDenied
        );
        let mut missing_identity = qualification(Some(&positive));
        missing_identity.source_run_id = "";
        assert_eq!(
            PromptLearningEligibilityReceiptV1::qualify(missing_identity).rejection,
            PromptLearningRejection::MissingIdentity
        );
    }

    #[test]
    fn teacher_requires_positive_complete_workflow_evidence() {
        let negative = evidence(LearningDisposition::Negative);
        let mut input = qualification(Some(&negative));
        input.purpose = PromptLearningPurpose::AutoTeacher;
        assert_eq!(
            PromptLearningEligibilityReceiptV1::qualify(input).rejection,
            PromptLearningRejection::TeacherNotPositive
        );
        let mut partial = evidence(LearningDisposition::Positive);
        partial.usage_completeness = LearningUsageCompleteness::Partial;
        let mut input = qualification(Some(&partial));
        input.purpose = PromptLearningPurpose::AutoTeacher;
        assert_eq!(
            PromptLearningEligibilityReceiptV1::qualify(input).rejection,
            PromptLearningRejection::TeacherUsageIncomplete
        );
    }

    #[test]
    fn dataset_identity_is_order_independent_and_split_isolation_is_strict() {
        let case = |id: &str, objective: &str, split| PromptDatasetCaseIdentityV1 {
            case_id: id.to_string(),
            objective_sha256: learning_sha256_hex(objective.as_bytes()),
            task_family_sha256: learning_sha256_hex(b"coding"),
            split,
        };
        let left = PromptDatasetIdentityV1::new(
            "project",
            3,
            vec![
                case("b", "second", PromptEvaluationSplit::Holdout),
                case("a", "first", PromptEvaluationSplit::Train),
            ],
        )
        .unwrap();
        let right = PromptDatasetIdentityV1::new(
            "project",
            3,
            vec![
                case("a", "first", PromptEvaluationSplit::Train),
                case("b", "second", PromptEvaluationSplit::Holdout),
            ],
        )
        .unwrap();
        assert_eq!(left, right);
        assert_ne!(left.train_sha256, left.holdout_sha256);
        assert!(PromptDatasetIdentityV1::new(
            "project",
            3,
            vec![
                case("a", "same", PromptEvaluationSplit::Train),
                case("b", "same", PromptEvaluationSplit::Holdout),
            ],
        )
        .is_err());
    }
}
