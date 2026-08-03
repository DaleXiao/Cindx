use super::learning_dataset::is_sha256;
use super::{PromptEvaluationMode, PromptEvaluationSplit, PromptLearningCohortV1};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROMPT_MATCHED_EVALUATION_SCHEMA_V1: &str = "cindx.prompt-matched-evaluation.v1";
pub const PROMPT_EVALUATION_ATTEMPT_SCHEMA_V1: &str = "cindx.prompt-evaluation-attempt.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptMatchedEvaluationIdentityV1 {
    pub schema: String,
    pub evaluation_id: String,
    pub cohort_sha256: String,
    pub dataset_sha256: String,
    pub case_id: String,
    pub objective_sha256: String,
    pub split: PromptEvaluationSplit,
    pub mode: PromptEvaluationMode,
}

impl PromptMatchedEvaluationIdentityV1 {
    pub fn new(
        evaluation_id: impl Into<String>,
        cohort: &PromptLearningCohortV1,
        case_id: impl Into<String>,
        split: PromptEvaluationSplit,
        mode: PromptEvaluationMode,
    ) -> Result<Self, String> {
        cohort.validate()?;
        let case_id = case_id.into();
        let case = cohort.dataset.case(&case_id).ok_or_else(|| {
            "matched evaluation case is absent from the frozen dataset".to_string()
        })?;
        if case.split != split {
            return Err("matched evaluation split differs from the frozen dataset".to_string());
        }
        let identity = Self {
            schema: PROMPT_MATCHED_EVALUATION_SCHEMA_V1.to_string(),
            evaluation_id: evaluation_id.into(),
            cohort_sha256: cohort.cohort_sha256.clone(),
            dataset_sha256: cohort.dataset.dataset_sha256.clone(),
            case_id,
            objective_sha256: case.objective_sha256.clone(),
            split,
            mode,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), String> {
        let mode_matches_split = match self.mode {
            PromptEvaluationMode::PairedExecution => self.split == PromptEvaluationSplit::Train,
            PromptEvaluationMode::ReplayExecution => self.split == PromptEvaluationSplit::Holdout,
            _ => false,
        };
        if self.schema != PROMPT_MATCHED_EVALUATION_SCHEMA_V1
            || self.evaluation_id.trim().is_empty()
            || self.evaluation_id.len() > 512
            || self.case_id.trim().is_empty()
            || !is_sha256(&self.cohort_sha256)
            || !is_sha256(&self.dataset_sha256)
            || !is_sha256(&self.objective_sha256)
            || !mode_matches_split
        {
            return Err("matched evaluation identity is malformed".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptTreatmentIdentityV1 {
    pub profile_id: String,
    pub prompt_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvaluationAttemptStatus {
    Started,
    CompletedPair,
    TreatmentFailure,
    InfrastructureInvalid,
    ForegroundPreempted,
}

impl PromptEvaluationAttemptStatus {
    pub fn is_terminal(self) -> bool {
        self != Self::Started
    }

    pub fn enters_effect_denominator(self) -> bool {
        matches!(self, Self::CompletedPair | Self::TreatmentFailure)
    }

    pub fn invalidates_comparison(self) -> bool {
        self == Self::InfrastructureInvalid
    }

    pub fn is_censored(self) -> bool {
        self == Self::ForegroundPreempted
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptEvaluationAttemptEventV1 {
    pub schema: String,
    pub identity: PromptMatchedEvaluationIdentityV1,
    pub treatments: [PromptTreatmentIdentityV1; 2],
    pub status: PromptEvaluationAttemptStatus,
    pub treatment_failures: [bool; 2],
    pub reason_code: String,
}

impl PromptEvaluationAttemptEventV1 {
    pub fn started(
        identity: PromptMatchedEvaluationIdentityV1,
        cohort: &PromptLearningCohortV1,
        treatments: [PromptTreatmentIdentityV1; 2],
    ) -> Result<Self, String> {
        cohort.validate()?;
        if identity.cohort_sha256 != cohort.cohort_sha256
            || identity.dataset_sha256 != cohort.dataset.dataset_sha256
        {
            return Err("prompt evaluation attempt cohort is mismatched".to_string());
        }
        Self::new(
            identity,
            treatments,
            PromptEvaluationAttemptStatus::Started,
            [false; 2],
            "",
        )
    }

    pub fn terminal(
        started: &Self,
        status: PromptEvaluationAttemptStatus,
        treatment_failures: [bool; 2],
        reason_code: &str,
    ) -> Result<Self, String> {
        if started.status != PromptEvaluationAttemptStatus::Started || !status.is_terminal() {
            return Err("prompt evaluation terminal does not follow a start".to_string());
        }
        Self::new(
            started.identity.clone(),
            started.treatments.clone(),
            status,
            treatment_failures,
            reason_code,
        )
    }

    fn new(
        identity: PromptMatchedEvaluationIdentityV1,
        treatments: [PromptTreatmentIdentityV1; 2],
        status: PromptEvaluationAttemptStatus,
        treatment_failures: [bool; 2],
        reason_code: &str,
    ) -> Result<Self, String> {
        let event = Self {
            schema: PROMPT_EVALUATION_ATTEMPT_SCHEMA_V1.to_string(),
            identity,
            treatments,
            status,
            treatment_failures,
            reason_code: reason_code.to_string(),
        };
        event.validate()?;
        Ok(event)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.identity.validate()?;
        if self.schema != PROMPT_EVALUATION_ATTEMPT_SCHEMA_V1
            || self.treatments.iter().any(|treatment| {
                treatment.profile_id.trim().is_empty()
                    || treatment.profile_id.len() > 256
                    || !is_sha256(&treatment.prompt_sha256)
            })
            || self.treatments[0].profile_id == self.treatments[1].profile_id
            || self.treatments[0].prompt_sha256 == self.treatments[1].prompt_sha256
        {
            return Err("prompt evaluation treatments are malformed".to_string());
        }
        let reason_is_safe = self.reason_code.len() <= 96
            && self.reason_code.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        let has_treatment_failure = self.treatment_failures.into_iter().any(|failed| failed);
        if !reason_is_safe
            || (self.status == PromptEvaluationAttemptStatus::Started && has_treatment_failure)
            || (self.status == PromptEvaluationAttemptStatus::CompletedPair
                && has_treatment_failure)
            || (self.status == PromptEvaluationAttemptStatus::TreatmentFailure
                && !has_treatment_failure)
            || (matches!(
                self.status,
                PromptEvaluationAttemptStatus::Started
                    | PromptEvaluationAttemptStatus::CompletedPair
            ) && !self.reason_code.is_empty())
            || (matches!(
                self.status,
                PromptEvaluationAttemptStatus::TreatmentFailure
                    | PromptEvaluationAttemptStatus::InfrastructureInvalid
                    | PromptEvaluationAttemptStatus::ForegroundPreempted
            ) && self.reason_code.is_empty())
        {
            return Err("prompt evaluation attempt reason is malformed".to_string());
        }
        Ok(())
    }

    pub fn enters_effect_denominator(&self) -> bool {
        self.status.enters_effect_denominator()
            || self.treatment_failures.into_iter().any(|failed| failed)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptEvaluationAttemptSummary {
    pub started: usize,
    pub completed_pairs: usize,
    pub treatment_failures: usize,
    pub infrastructure_invalid: usize,
    pub censored: usize,
}

pub fn validate_prompt_evaluation_attempt_ledger(
    events: &[PromptEvaluationAttemptEventV1],
) -> Result<PromptEvaluationAttemptSummary, String> {
    let mut attempts = BTreeMap::<&str, (&PromptEvaluationAttemptEventV1, bool)>::new();
    let mut summary = PromptEvaluationAttemptSummary::default();
    for event in events {
        event.validate()?;
        let key = event.identity.evaluation_id.as_str();
        if event.status == PromptEvaluationAttemptStatus::Started {
            if attempts.insert(key, (event, false)).is_some() {
                return Err("duplicate prompt evaluation attempt start".to_string());
            }
            summary.started += 1;
            continue;
        }
        let Some((started, terminal_seen)) = attempts.get_mut(key) else {
            return Err("orphan prompt evaluation attempt terminal".to_string());
        };
        if *terminal_seen
            || started.identity != event.identity
            || started.treatments != event.treatments
        {
            return Err(
                "prompt evaluation attempt terminal is duplicated or mismatched".to_string(),
            );
        }
        *terminal_seen = true;
        summary.treatment_failures += event
            .treatment_failures
            .into_iter()
            .filter(|failed| *failed)
            .count();
        match event.status {
            PromptEvaluationAttemptStatus::CompletedPair => summary.completed_pairs += 1,
            PromptEvaluationAttemptStatus::TreatmentFailure => {}
            PromptEvaluationAttemptStatus::InfrastructureInvalid => {
                summary.infrastructure_invalid += 1
            }
            PromptEvaluationAttemptStatus::ForegroundPreempted => summary.censored += 1,
            PromptEvaluationAttemptStatus::Started => unreachable!(),
        }
    }
    if attempts.values().any(|(_, terminal_seen)| !terminal_seen) {
        return Err("prompt evaluation attempt is missing a terminal".to_string());
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cohort() -> PromptLearningCohortV1 {
        let dataset = super::super::PromptDatasetIdentityV1::new(
            "project",
            1,
            vec![super::super::PromptDatasetCaseIdentityV1 {
                case_id: "case-1".to_string(),
                objective_sha256: "c".repeat(64),
                task_family_sha256: "f".repeat(64),
                split: PromptEvaluationSplit::Train,
            }],
        )
        .unwrap();
        let execution = super::super::PromptExecutionContextV1 {
            schema: super::super::PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            harness_sha256: "3".repeat(64),
            system_prompt_sha256: "4".repeat(64),
            policy: crate::AgentPolicy::Auto,
            policy_sha256: "5".repeat(64),
            budget_sha256: "6".repeat(64),
            tool_contract_sha256: "7".repeat(64),
            source_revision_sha256: "8".repeat(64),
            workspace_revision_sha256: "9".repeat(64),
        };
        PromptLearningCohortV1::new(dataset, execution).unwrap()
    }

    fn identity(cohort: &PromptLearningCohortV1) -> PromptMatchedEvaluationIdentityV1 {
        PromptMatchedEvaluationIdentityV1::new(
            "project::attempt-1",
            cohort,
            "case-1",
            PromptEvaluationSplit::Train,
            PromptEvaluationMode::PairedExecution,
        )
        .unwrap()
    }

    fn treatments() -> [PromptTreatmentIdentityV1; 2] {
        [
            PromptTreatmentIdentityV1 {
                profile_id: "stable".to_string(),
                prompt_sha256: "d".repeat(64),
            },
            PromptTreatmentIdentityV1 {
                profile_id: "candidate".to_string(),
                prompt_sha256: "e".repeat(64),
            },
        ]
    }

    #[test]
    fn every_started_attempt_requires_exactly_one_matching_terminal() {
        let cohort = cohort();
        let started =
            PromptEvaluationAttemptEventV1::started(identity(&cohort), &cohort, treatments())
                .unwrap();
        assert!(validate_prompt_evaluation_attempt_ledger(std::slice::from_ref(&started)).is_err());
        let terminal = PromptEvaluationAttemptEventV1::terminal(
            &started,
            PromptEvaluationAttemptStatus::InfrastructureInvalid,
            [false; 2],
            "reviewer_invalid_json",
        )
        .unwrap();
        let summary =
            validate_prompt_evaluation_attempt_ledger(&[started.clone(), terminal.clone()])
                .unwrap();
        assert_eq!(summary.started, 1);
        assert_eq!(summary.infrastructure_invalid, 1);
        assert!(
            validate_prompt_evaluation_attempt_ledger(&[started, terminal.clone(), terminal])
                .is_err()
        );
    }

    #[test]
    fn treatment_failure_is_counted_and_preemption_is_explicitly_censored() {
        let cohort = cohort();
        let started =
            PromptEvaluationAttemptEventV1::started(identity(&cohort), &cohort, treatments())
                .unwrap();
        let failed = PromptEvaluationAttemptEventV1::terminal(
            &started,
            PromptEvaluationAttemptStatus::TreatmentFailure,
            [true, false],
            "candidate_failed",
        )
        .unwrap();
        let summary = validate_prompt_evaluation_attempt_ledger(&[started, failed]).unwrap();
        assert_eq!(summary.treatment_failures, 1);
        assert_eq!(summary.completed_pairs, 0);

        let mut second_identity = identity(&cohort);
        second_identity.evaluation_id = "project::attempt-2".to_string();
        let second =
            PromptEvaluationAttemptEventV1::started(second_identity, &cohort, treatments())
                .unwrap();
        let censored = PromptEvaluationAttemptEventV1::terminal(
            &second,
            PromptEvaluationAttemptStatus::ForegroundPreempted,
            [false; 2],
            "foreground_preempted",
        )
        .unwrap();
        let summary = validate_prompt_evaluation_attempt_ledger(&[second, censored]).unwrap();
        assert_eq!(summary.censored, 1);
        assert_eq!(summary.treatment_failures, 0);
    }

    #[test]
    fn attempt_storage_is_bounded_independently_of_dataset_size() {
        let mut cases = Vec::new();
        for index in 0..1_000 {
            cases.push(super::super::PromptDatasetCaseIdentityV1 {
                case_id: format!("case-{index:04}"),
                objective_sha256: crate::sha256_hex(format!("objective-{index}").as_bytes()),
                task_family_sha256: crate::sha256_hex(format!("family-{index}").as_bytes()),
                split: if index % 2 == 0 {
                    PromptEvaluationSplit::Train
                } else {
                    PromptEvaluationSplit::Holdout
                },
            });
        }
        let dataset = super::super::PromptDatasetIdentityV1::new("project", 1, cases).unwrap();
        let execution = cohort().execution;
        let cohort = PromptLearningCohortV1::new(dataset, execution).unwrap();
        let identity = PromptMatchedEvaluationIdentityV1::new(
            "project::large-attempt",
            &cohort,
            "case-0000",
            PromptEvaluationSplit::Train,
            PromptEvaluationMode::PairedExecution,
        )
        .unwrap();
        let started =
            PromptEvaluationAttemptEventV1::started(identity, &cohort, treatments()).unwrap();
        assert!(serde_json::to_vec(&started).unwrap().len() < 2_048);
    }
}
