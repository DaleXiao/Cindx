use crate::direct_judge_fitness::{summarize_direct_judge_fitness, DirectJudgeFitnessSignalV1};
use crate::direct_judge_outcome::DirectJudgeOutcomeError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DIRECT_JUDGE_REVIEW_RECEIPT_SCHEMA: &str = "cindx.agent.direct-judge-review-receipt.v1";
pub const DIRECT_JUDGE_FITNESS_ADMISSION_SCHEMA: &str =
    "cindx.agent.direct-judge-fitness-admission.v1";

const DIRECT_JUDGE_REVIEW_HASH_DOMAIN: &[u8] = b"cindx.agent.direct-judge-review-receipt.v1\0";
const DIRECT_JUDGE_FITNESS_ADMISSION_HASH_DOMAIN: &[u8] =
    b"cindx.agent.direct-judge-fitness-admission.v1\0";
const DIRECT_JUDGE_WINDOW_HASH_DOMAIN: &[u8] = b"cindx.agent.direct-judge-fitness-window.v1\0";
const MAX_DIRECT_JUDGE_ADMISSION_JSON_BYTES: usize = 8 * 1024;

/// Independently issued review receipt over one bounded fitness-signal
/// window. Mirrors the collaboration-learning review receipt: the reviewer
/// binds the exact window digest and an explicit decision; nothing else can
/// admit the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectJudgeReviewReceiptV1 {
    pub schema: String,
    pub reviewer_identity_sha256: String,
    pub window_digest_sha256: String,
    pub window_size: usize,
    pub approve: bool,
    pub review_sha256: String,
}

impl DirectJudgeReviewReceiptV1 {
    pub fn new(
        reviewer_identity_sha256: String,
        window_digest_sha256: String,
        window_size: usize,
        approve: bool,
    ) -> Result<Self, DirectJudgeOutcomeError> {
        let mut receipt = Self {
            schema: DIRECT_JUDGE_REVIEW_RECEIPT_SCHEMA.to_string(),
            reviewer_identity_sha256,
            window_digest_sha256,
            window_size,
            approve,
            review_sha256: String::new(),
        };
        receipt.validate_payload()?;
        receipt.review_sha256 = receipt.payload_sha256()?;
        Ok(receipt)
    }

    pub fn from_json(encoded: &str) -> Result<Self, DirectJudgeOutcomeError> {
        if encoded.len() > MAX_DIRECT_JUDGE_ADMISSION_JSON_BYTES {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge review receipt JSON exceeds its size bound",
            ));
        }
        let receipt = serde_json::from_str::<Self>(encoded).map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge review receipt JSON is invalid: {error}"
            ))
        })?;
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn to_json(&self) -> Result<String, DirectJudgeOutcomeError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge review receipt JSON encoding failed: {error}"
            ))
        })
    }

    pub fn validate(&self) -> Result<(), DirectJudgeOutcomeError> {
        self.validate_payload()?;
        let expected = self.payload_sha256()?;
        if self.review_sha256 != expected {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge review receipt digest is invalid",
            ));
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DirectJudgeOutcomeError> {
        if self.schema != DIRECT_JUDGE_REVIEW_RECEIPT_SCHEMA {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge review receipt schema is unsupported",
            ));
        }
        validate_digest(&self.reviewer_identity_sha256, "reviewer identity")?;
        validate_digest(&self.window_digest_sha256, "window digest")?;
        if self.window_size == 0 {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge review receipt must bind a non-empty window",
            ));
        }
        Ok(())
    }

    fn payload_sha256(&self) -> Result<String, DirectJudgeOutcomeError> {
        let payload = serde_json::to_vec(&(
            self.schema.as_str(),
            self.reviewer_identity_sha256.as_str(),
            self.window_digest_sha256.as_str(),
            self.window_size,
            self.approve,
        ))
        .map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge review receipt digest encoding failed: {error}"
            ))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(DIRECT_JUDGE_REVIEW_HASH_DOMAIN);
        hasher.update(payload);
        Ok(hex_digest(hasher.finalize().as_slice()))
    }
}

/// Order- and content-binding digest of one fitness-signal window. Any
/// reordered, replaced, appended, or dropped signal changes it.
pub fn direct_judge_fitness_window_digest(
    signals: &[DirectJudgeFitnessSignalV1],
) -> Result<String, DirectJudgeOutcomeError> {
    if signals.is_empty() {
        return Err(DirectJudgeOutcomeError::new(
            "direct judge fitness window is empty",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(DIRECT_JUDGE_WINDOW_HASH_DOMAIN);
    hasher.update(signals.len().to_be_bytes());
    for signal in signals {
        signal.validate()?;
        hasher.update(b"\0");
        hasher.update(signal.outcome_receipt_sha256.as_bytes());
    }
    Ok(hex_digest(hasher.finalize().as_slice()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectJudgeFitnessAdmissionV1 {
    pub schema: String,
    pub review_sha256: String,
    pub reviewer_identity_sha256: String,
    pub window_digest_sha256: String,
    pub window_size: usize,
    pub scored_runs: usize,
    pub censored_runs: usize,
    pub passed_runs: usize,
    pub revise_exhausted_runs: usize,
    pub fail_open_runs: usize,
    pub average_reward_bps: u16,
    pub mutation_verified_rate_bps: Option<u16>,
    pub production_eligible: bool,
    pub promotion_eligible: bool,
    pub admission_sha256: String,
}

impl DirectJudgeFitnessAdmissionV1 {
    pub fn from_json(encoded: &str) -> Result<Self, DirectJudgeOutcomeError> {
        if encoded.len() > MAX_DIRECT_JUDGE_ADMISSION_JSON_BYTES {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge fitness admission JSON exceeds its size bound",
            ));
        }
        let admission = serde_json::from_str::<Self>(encoded).map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge fitness admission JSON is invalid: {error}"
            ))
        })?;
        admission.validate()?;
        Ok(admission)
    }

    pub fn to_json(&self) -> Result<String, DirectJudgeOutcomeError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge fitness admission JSON encoding failed: {error}"
            ))
        })
    }

    pub fn validate(&self) -> Result<(), DirectJudgeOutcomeError> {
        self.validate_payload()?;
        let expected = Self::derive_admission_digest(self)?;
        if self.admission_sha256 != expected {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge fitness admission digest is invalid",
            ));
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DirectJudgeOutcomeError> {
        if self.schema != DIRECT_JUDGE_FITNESS_ADMISSION_SCHEMA {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge fitness admission schema is unsupported",
            ));
        }
        validate_digest(&self.review_sha256, "review receipt")?;
        validate_digest(&self.reviewer_identity_sha256, "reviewer identity")?;
        validate_digest(&self.window_digest_sha256, "window digest")?;
        if self.window_size == 0 {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge fitness admission must bind a non-empty window",
            ));
        }
        if self.scored_runs == 0
            || self.scored_runs + self.censored_runs != self.window_size
            || self.passed_runs + self.revise_exhausted_runs + self.fail_open_runs
                != self.scored_runs
        {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge fitness admission counters are inconsistent",
            ));
        }
        if self.average_reward_bps > 10_000 {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge fitness admission reward exceeds its bound",
            ));
        }
        if let Some(rate) = self.mutation_verified_rate_bps {
            if rate > 10_000 {
                return Err(DirectJudgeOutcomeError::new(
                    "direct judge fitness admission verification rate exceeds its bound",
                ));
            }
        }
        if self.production_eligible || self.promotion_eligible {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge fitness admission must stay production- and promotion-ineligible",
            ));
        }
        Ok(())
    }

    fn derive_admission_digest(admission: &Self) -> Result<String, DirectJudgeOutcomeError> {
        let payload = serde_json::to_vec(&(
            admission.schema.as_str(),
            admission.review_sha256.as_str(),
            admission.reviewer_identity_sha256.as_str(),
            admission.window_digest_sha256.as_str(),
            admission.window_size,
            admission.scored_runs,
            admission.censored_runs,
            admission.passed_runs,
            admission.revise_exhausted_runs,
            admission.fail_open_runs,
            admission.average_reward_bps,
            admission.mutation_verified_rate_bps,
            admission.production_eligible,
            admission.promotion_eligible,
        ))
        .map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge fitness admission digest encoding failed: {error}"
            ))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(DIRECT_JUDGE_FITNESS_ADMISSION_HASH_DOMAIN);
        hasher.update(payload);
        Ok(hex_digest(hasher.finalize().as_slice()))
    }
}

/// Admits one bounded signal window into fitness consumption only when the
/// review receipt binds exactly that window and approves it. Admission never
/// grants production or promotion eligibility; promotion stays gated on
/// scientific paired evidence elsewhere.
pub fn admit_direct_judge_fitness_window(
    signals: &[DirectJudgeFitnessSignalV1],
    receipt: &DirectJudgeReviewReceiptV1,
) -> Result<DirectJudgeFitnessAdmissionV1, DirectJudgeOutcomeError> {
    receipt.validate()?;
    if !receipt.approve {
        return Err(DirectJudgeOutcomeError::new(
            "direct judge review receipt does not admit the window",
        ));
    }
    let window_digest = direct_judge_fitness_window_digest(signals)?;
    if window_digest != receipt.window_digest_sha256 {
        return Err(DirectJudgeOutcomeError::new(
            "direct judge review receipt does not bind this signal window",
        ));
    }
    if signals.len() != receipt.window_size {
        return Err(DirectJudgeOutcomeError::new(
            "direct judge review receipt window size disagrees with the signals",
        ));
    }
    let summary = summarize_direct_judge_fitness(signals)?;
    let average_reward_bps = summary.average_reward_bps.ok_or_else(|| {
        DirectJudgeOutcomeError::new("direct judge fitness window has no scored runs to admit")
    })?;
    let mut admission = DirectJudgeFitnessAdmissionV1 {
        schema: DIRECT_JUDGE_FITNESS_ADMISSION_SCHEMA.to_string(),
        review_sha256: receipt.review_sha256.clone(),
        reviewer_identity_sha256: receipt.reviewer_identity_sha256.clone(),
        window_digest_sha256: window_digest,
        window_size: summary.window,
        scored_runs: summary.scored_runs,
        censored_runs: summary.censored_runs,
        passed_runs: summary.passed_runs,
        revise_exhausted_runs: summary.revise_exhausted_runs,
        fail_open_runs: summary.fail_open_runs,
        average_reward_bps,
        mutation_verified_rate_bps: summary.mutation_verified_rate_bps,
        production_eligible: false,
        promotion_eligible: false,
        admission_sha256: String::new(),
    };
    admission.validate_payload()?;
    admission.admission_sha256 =
        DirectJudgeFitnessAdmissionV1::derive_admission_digest(&admission)?;
    Ok(admission)
}

fn validate_digest(value: &str, label: &str) -> Result<(), DirectJudgeOutcomeError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DirectJudgeOutcomeError::new(format!(
            "direct judge admission {label} is not a SHA-256 digest"
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
    use crate::direct_judge_outcome::{DirectJudgeCompletionFacts, DirectJudgeOutcomeV1};

    fn signal(task: &str, disposition: &'static str, verified: bool) -> DirectJudgeFitnessSignalV1 {
        let outcome = DirectJudgeOutcomeV1::from_completion_facts(&DirectJudgeCompletionFacts {
            task_identity: task,
            disposition,
            successful_mutations: 1,
            latest_mutation_verified: verified,
            workspace_verification_required: true,
            grounded_basis: "postcondition_verified",
        })
        .expect("outcome projects");
        DirectJudgeFitnessSignalV1::from_outcome(&outcome).expect("signal projects")
    }

    fn reviewer() -> String {
        "a".repeat(64)
    }

    fn window(verified_count: usize, censored_count: usize) -> Vec<DirectJudgeFitnessSignalV1> {
        let mut signals = Vec::new();
        for index in 0..verified_count {
            signals.push(signal(
                &format!("task-v{index}"),
                "direct_judge_passed",
                true,
            ));
        }
        for index in 0..censored_count {
            signals.push(signal(
                &format!("task-c{index}"),
                "direct_judge_not_applicable",
                true,
            ));
        }
        signals
    }

    #[test]
    fn direct_judge_outcome_contract_admission_requires_an_approving_bound_receipt() {
        println!("{DIRECT_JUDGE_FITNESS_ADMISSION_SCHEMA}");
        let signals = window(3, 1);
        let digest = direct_judge_fitness_window_digest(&signals).expect("digest");
        let receipt =
            DirectJudgeReviewReceiptV1::new(reviewer(), digest.clone(), signals.len(), true)
                .expect("receipt");
        let admission = admit_direct_judge_fitness_window(&signals, &receipt).expect("admits");
        assert_eq!(admission.window_size, 4);
        assert_eq!(admission.scored_runs, 3);
        assert_eq!(admission.censored_runs, 1);
        assert_eq!(admission.passed_runs, 3);
        assert_eq!(admission.average_reward_bps, 10_000);
        assert!(admission.mutation_verified_rate_bps.is_some());
        assert!(!admission.production_eligible);
        assert!(!admission.promotion_eligible);
        admission.validate().expect("admission validates");
        let decoded =
            DirectJudgeFitnessAdmissionV1::from_json(&admission.to_json().expect("encodes"))
                .expect("roundtrip");
        assert_eq!(decoded, admission);
    }

    #[test]
    fn direct_judge_outcome_contract_admission_fails_closed_on_any_drift() {
        let signals = window(2, 0);
        let digest = direct_judge_fitness_window_digest(&signals).expect("digest");

        let rejecting =
            DirectJudgeReviewReceiptV1::new(reviewer(), digest.clone(), signals.len(), false)
                .expect("receipt");
        assert!(admit_direct_judge_fitness_window(&signals, &rejecting).is_err());

        let wrong_size =
            DirectJudgeReviewReceiptV1::new(reviewer(), digest.clone(), signals.len() + 1, true)
                .expect("receipt");
        assert!(admit_direct_judge_fitness_window(&signals, &wrong_size).is_err());

        let foreign_digest =
            DirectJudgeReviewReceiptV1::new(reviewer(), "b".repeat(64), signals.len(), true)
                .expect("receipt");
        assert!(admit_direct_judge_fitness_window(&signals, &foreign_digest).is_err());

        let mut reordered = signals.clone();
        reordered.reverse();
        let reordered_digest = direct_judge_fitness_window_digest(&reordered).expect("digest");
        assert_ne!(reordered_digest, digest);

        let mut replaced = signals.clone();
        replaced[0] = signal("task-other", "direct_judge_passed", true);
        assert!(admit_direct_judge_fitness_window(&replaced, &receipt_for(&signals)).is_err());
    }

    fn receipt_for(signals: &[DirectJudgeFitnessSignalV1]) -> DirectJudgeReviewReceiptV1 {
        let digest = direct_judge_fitness_window_digest(signals).expect("digest");
        DirectJudgeReviewReceiptV1::new(reviewer(), digest, signals.len(), true).expect("receipt")
    }

    #[test]
    fn direct_judge_outcome_contract_review_receipt_rejects_tamper_and_empty_windows() {
        assert!(direct_judge_fitness_window_digest(&[]).is_err());

        let receipt =
            DirectJudgeReviewReceiptV1::new(reviewer(), "c".repeat(64), 3, true).expect("receipt");
        let decoded = DirectJudgeReviewReceiptV1::from_json(&receipt.to_json().expect("encodes"))
            .expect("roundtrip");
        assert_eq!(decoded, receipt);

        let mut tampered: serde_json::Value =
            serde_json::from_str(&receipt.to_json().expect("encodes")).expect("value");
        tampered["approve"] = serde_json::json!(false);
        let tampered_encoded = serde_json::to_string(&tampered).expect("encodes tamper");
        assert!(DirectJudgeReviewReceiptV1::from_json(&tampered_encoded).is_err());

        assert!(DirectJudgeReviewReceiptV1::new(reviewer(), "c".repeat(64), 0, true).is_err());
        assert!(DirectJudgeReviewReceiptV1::new(
            "not-a-digest".to_string(),
            "c".repeat(64),
            1,
            true
        )
        .is_err());
        println!("{DIRECT_JUDGE_REVIEW_RECEIPT_SCHEMA}");
    }

    #[test]
    fn direct_judge_outcome_contract_admission_rejects_unscored_windows() {
        let signals = window(0, 3);
        let receipt = receipt_for(&signals);
        let error = admit_direct_judge_fitness_window(&signals, &receipt)
            .expect_err("all-censored windows carry no reward to admit");
        assert!(error.to_string().contains("no scored runs"));
    }
}
