use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

pub const DIRECT_JUDGE_OUTCOME_SCHEMA: &str = "cindx.agent.direct-judge-outcome.v1";

const DIRECT_JUDGE_OUTCOME_HASH_DOMAIN: &[u8] = b"cindx.agent.direct-judge-outcome.v1\0";
const MAX_DIRECT_JUDGE_OUTCOME_JSON_BYTES: usize = 16 * 1024;

pub const DIRECT_JUDGE_DISPOSITION_PASSED: &str = "direct_judge_passed";
pub const DIRECT_JUDGE_DISPOSITION_RECHECK_PASSED: &str = "direct_judge_recheck_passed";
pub const DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED: &str = "direct_judge_recheck_exhausted";
pub const DIRECT_JUDGE_DISPOSITION_RECHECK_INCONCLUSIVE: &str = "direct_judge_recheck_inconclusive";
pub const DIRECT_JUDGE_DISPOSITION_NOT_APPLICABLE: &str = "direct_judge_not_applicable";
pub const DIRECT_JUDGE_DISPOSITION_NOT_ELIGIBLE: &str = "direct_judge_not_eligible";
pub const DIRECT_JUDGE_DISPOSITION_UNAVAILABLE: &str = "direct_judge_unavailable";
pub const DIRECT_JUDGE_DISPOSITION_INCONCLUSIVE: &str = "direct_judge_inconclusive";
pub const DIRECT_JUDGE_DISPOSITION_REPAIR_UNAVAILABLE: &str = "direct_judge_repair_unavailable";
pub const DIRECT_JUDGE_DISPOSITION_REPAIR_EMPTY: &str = "direct_judge_repair_empty";
pub const DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED: &str = "direct_judge_repair_ungrounded";
/// The judge required revision, the repair loop closed without a pass, and the
/// run was configured fail-closed, so the candidate was not delivered.
pub const DIRECT_JUDGE_DISPOSITION_FAIL_CLOSED_BLOCKED: &str = "direct_judge_fail_closed_blocked";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectJudgeOutcomeError(String);

impl DirectJudgeOutcomeError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for DirectJudgeOutcomeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DirectJudgeOutcomeError {}

/// Coarse semantic family of a recorded `direct_judge_disposition`. The
/// string itself is retained on the receipt for auditability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectJudgeDispositionFamilyV1 {
    Passed,
    ReviseExhausted,
    FailOpen,
    NotJudged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectJudgeMutationVerificationV1 {
    NoMutations,
    VerifiedAfterLastMutation,
    UnverifiedAfterMutation,
}

/// Provider-free completion facts available at terminal finalization. The
/// desktop adapter fills these from the task contract and grounded receipt;
/// the reward derivation never trusts a field it cannot validate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectJudgeCompletionFacts<'a> {
    pub task_identity: &'a str,
    pub disposition: &'a str,
    pub successful_mutations: u64,
    pub latest_mutation_verified: bool,
    pub workspace_verification_required: bool,
    pub grounded_basis: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectJudgeOutcomeV1 {
    pub schema: String,
    pub task_identity_sha256: String,
    pub disposition: String,
    pub family: DirectJudgeDispositionFamilyV1,
    pub reviewer_independent: bool,
    pub repair_round_used: bool,
    pub mutation_verification: DirectJudgeMutationVerificationV1,
    pub workspace_verification_required: bool,
    pub grounded_basis: String,
    pub successful_mutations: u64,
    pub reward_bps: Option<u16>,
    pub receipt_sha256: String,
}

impl DirectJudgeOutcomeV1 {
    pub fn from_completion_facts(
        facts: &DirectJudgeCompletionFacts<'_>,
    ) -> Result<Self, DirectJudgeOutcomeError> {
        if facts.task_identity.trim().is_empty() {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome task identity is missing",
            ));
        }
        let family = disposition_family(facts.disposition)?;
        let mutation_verification = if facts.successful_mutations == 0 {
            DirectJudgeMutationVerificationV1::NoMutations
        } else if facts.latest_mutation_verified {
            DirectJudgeMutationVerificationV1::VerifiedAfterLastMutation
        } else {
            DirectJudgeMutationVerificationV1::UnverifiedAfterMutation
        };
        let mut outcome = Self {
            schema: DIRECT_JUDGE_OUTCOME_SCHEMA.to_string(),
            task_identity_sha256: task_identity_digest(facts.task_identity),
            disposition: facts.disposition.to_string(),
            family,
            reviewer_independent: family != DirectJudgeDispositionFamilyV1::NotJudged,
            repair_round_used: disposition_used_repair_round(facts.disposition),
            mutation_verification,
            workspace_verification_required: facts.workspace_verification_required,
            grounded_basis: facts.grounded_basis.to_string(),
            successful_mutations: facts.successful_mutations,
            reward_bps: None,
            receipt_sha256: String::new(),
        };
        outcome.reward_bps = outcome.derive_reward();
        outcome.validate_payload()?;
        outcome.receipt_sha256 = outcome.payload_sha256()?;
        Ok(outcome)
    }

    /// Integer reward rule. Only an explicit judge pass earns positive
    /// credit; verification facts modulate its magnitude. Judged runs that
    /// did not pass remain zero-score evidence. Runs that never received a
    /// judge are censored (no reward value) so they stay out of any fitness
    /// denominator.
    fn derive_reward(&self) -> Option<u16> {
        match self.family {
            DirectJudgeDispositionFamilyV1::NotJudged => None,
            DirectJudgeDispositionFamilyV1::Passed => {
                let penalized = self.workspace_verification_required
                    && self.mutation_verification
                        == DirectJudgeMutationVerificationV1::UnverifiedAfterMutation;
                Some(if penalized { 5_000 } else { 10_000 })
            }
            DirectJudgeDispositionFamilyV1::ReviseExhausted
            | DirectJudgeDispositionFamilyV1::FailOpen => Some(0),
        }
    }

    pub fn censored(&self) -> bool {
        self.reward_bps.is_none()
    }

    pub fn from_json(encoded: &str) -> Result<Self, DirectJudgeOutcomeError> {
        if encoded.len() > MAX_DIRECT_JUDGE_OUTCOME_JSON_BYTES {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome JSON exceeds its size bound",
            ));
        }
        let outcome = serde_json::from_str::<Self>(encoded).map_err(|error| {
            DirectJudgeOutcomeError::new(format!("direct judge outcome JSON is invalid: {error}"))
        })?;
        outcome.validate()?;
        Ok(outcome)
    }

    pub fn to_json(&self) -> Result<String, DirectJudgeOutcomeError> {
        self.validate()?;
        let encoded = serde_json::to_string(self).map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge outcome JSON encoding failed: {error}"
            ))
        })?;
        if encoded.len() > MAX_DIRECT_JUDGE_OUTCOME_JSON_BYTES {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome JSON exceeds its size bound",
            ));
        }
        Ok(encoded)
    }

    pub fn validate(&self) -> Result<(), DirectJudgeOutcomeError> {
        self.validate_payload()?;
        let expected = self.payload_sha256()?;
        if self.receipt_sha256 != expected {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome receipt digest is invalid",
            ));
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DirectJudgeOutcomeError> {
        if self.schema != DIRECT_JUDGE_OUTCOME_SCHEMA {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome schema is unsupported",
            ));
        }
        validate_digest(&self.task_identity_sha256, "task identity")?;
        disposition_family(&self.disposition)?;
        if disposition_family(&self.disposition)? != self.family {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome family disagrees with its disposition",
            ));
        }
        if disposition_used_repair_round(&self.disposition) != self.repair_round_used {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome repair flag disagrees with its disposition",
            ));
        }
        if (self.family != DirectJudgeDispositionFamilyV1::NotJudged) != self.reviewer_independent {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome reviewer independence disagrees with its family",
            ));
        }
        match self.grounded_basis.as_str() {
            "postcondition_verified"
            | "evidence_visible"
            | "constraint_observed"
            | "self_contained" => {}
            other => {
                return Err(DirectJudgeOutcomeError::new(format!(
                    "direct judge outcome grounded basis is unsupported: {other}"
                )))
            }
        }
        if self.mutation_verification == DirectJudgeMutationVerificationV1::NoMutations
            && self.successful_mutations != 0
        {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome mutation state is inconsistent",
            ));
        }
        if self.mutation_verification != DirectJudgeMutationVerificationV1::NoMutations
            && self.successful_mutations == 0
        {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome mutation state is inconsistent",
            ));
        }
        let expected_reward = self.derive_reward();
        if self.reward_bps != expected_reward {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge outcome reward disagrees with its integer rule",
            ));
        }
        if let Some(reward) = self.reward_bps {
            if reward > 10_000 {
                return Err(DirectJudgeOutcomeError::new(
                    "direct judge outcome reward exceeds its bound",
                ));
            }
        }
        Ok(())
    }

    fn payload_sha256(&self) -> Result<String, DirectJudgeOutcomeError> {
        let payload = serde_json::to_vec(&(
            self.schema.as_str(),
            self.task_identity_sha256.as_str(),
            self.disposition.as_str(),
            self.family,
            self.reviewer_independent,
            self.repair_round_used,
            self.mutation_verification,
            self.workspace_verification_required,
            self.grounded_basis.as_str(),
            self.successful_mutations,
            self.reward_bps,
        ))
        .map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge outcome digest encoding failed: {error}"
            ))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(DIRECT_JUDGE_OUTCOME_HASH_DOMAIN);
        hasher.update(payload);
        Ok(hex_digest(hasher.finalize().as_slice()))
    }
}

pub fn disposition_family(
    disposition: &str,
) -> Result<DirectJudgeDispositionFamilyV1, DirectJudgeOutcomeError> {
    match disposition {
        DIRECT_JUDGE_DISPOSITION_PASSED | DIRECT_JUDGE_DISPOSITION_RECHECK_PASSED => {
            Ok(DirectJudgeDispositionFamilyV1::Passed)
        }
        DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED
        | DIRECT_JUDGE_DISPOSITION_FAIL_CLOSED_BLOCKED => {
            Ok(DirectJudgeDispositionFamilyV1::ReviseExhausted)
        }
        DIRECT_JUDGE_DISPOSITION_RECHECK_INCONCLUSIVE
        | DIRECT_JUDGE_DISPOSITION_UNAVAILABLE
        | DIRECT_JUDGE_DISPOSITION_INCONCLUSIVE
        | DIRECT_JUDGE_DISPOSITION_REPAIR_UNAVAILABLE
        | DIRECT_JUDGE_DISPOSITION_REPAIR_EMPTY
        | DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED => {
            Ok(DirectJudgeDispositionFamilyV1::FailOpen)
        }
        DIRECT_JUDGE_DISPOSITION_NOT_APPLICABLE | DIRECT_JUDGE_DISPOSITION_NOT_ELIGIBLE => {
            Ok(DirectJudgeDispositionFamilyV1::NotJudged)
        }
        other => Err(DirectJudgeOutcomeError::new(format!(
            "direct judge disposition is unknown: {other}"
        ))),
    }
}

pub fn disposition_used_repair_round(disposition: &str) -> bool {
    matches!(
        disposition,
        DIRECT_JUDGE_DISPOSITION_RECHECK_PASSED
            | DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED
            | DIRECT_JUDGE_DISPOSITION_RECHECK_INCONCLUSIVE
            | DIRECT_JUDGE_DISPOSITION_REPAIR_UNAVAILABLE
            | DIRECT_JUDGE_DISPOSITION_REPAIR_EMPTY
            | DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED
            | DIRECT_JUDGE_DISPOSITION_FAIL_CLOSED_BLOCKED
    )
}

fn task_identity_digest(task_identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DIRECT_JUDGE_OUTCOME_HASH_DOMAIN);
    hasher.update(b"task\0");
    hasher.update(task_identity.as_bytes());
    hex_digest(hasher.finalize().as_slice())
}

fn validate_digest(value: &str, label: &str) -> Result<(), DirectJudgeOutcomeError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DirectJudgeOutcomeError::new(format!(
            "direct judge outcome {label} is not a SHA-256 digest"
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

/// Fail-closed delivery decision for the judge gate. Only a judged quality
/// failure on a mutation-bearing run blocks delivery: the recheck still required
/// revision, or the judge required revision and the repair could not be grounded
/// against the task contract. Judge unavailability, inconclusive receipts, and
/// repair transport failures stay fail-open — they are infrastructure failures,
/// not quality evidence. Portable so the evaluation harness applies the same
/// fail-closed rule as the product (Phase 4 fidelity, audit 3b).
pub fn direct_judge_fail_closed_block(
    fail_closed_enabled: bool,
    successful_mutations: usize,
    disposition: &str,
    findings: &[String],
) -> Option<String> {
    if !fail_closed_enabled || successful_mutations == 0 {
        return None;
    }
    let detail = match disposition {
        DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED => {
            "the judge still requires revision after the single repair round"
        }
        DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED => {
            "the repair could not be grounded against the task contract"
        }
        _ => return None,
    };
    let findings_text = if findings.is_empty() {
        "(no findings recorded)".to_string()
    } else {
        findings.join("; ")
    };
    Some(format!(
        "Delivery verification failed: {detail}. The candidate answer was not delivered. \
         Judge findings: {findings_text}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(disposition: &'static str) -> DirectJudgeCompletionFacts<'static> {
        DirectJudgeCompletionFacts {
            task_identity: "task-1",
            disposition,
            successful_mutations: 2,
            latest_mutation_verified: true,
            workspace_verification_required: true,
            grounded_basis: "postcondition_verified",
        }
    }

    const ALL_DISPOSITIONS: [&str; 12] = [
        DIRECT_JUDGE_DISPOSITION_PASSED,
        DIRECT_JUDGE_DISPOSITION_RECHECK_PASSED,
        DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
        DIRECT_JUDGE_DISPOSITION_RECHECK_INCONCLUSIVE,
        DIRECT_JUDGE_DISPOSITION_NOT_APPLICABLE,
        DIRECT_JUDGE_DISPOSITION_NOT_ELIGIBLE,
        DIRECT_JUDGE_DISPOSITION_UNAVAILABLE,
        DIRECT_JUDGE_DISPOSITION_INCONCLUSIVE,
        DIRECT_JUDGE_DISPOSITION_REPAIR_UNAVAILABLE,
        DIRECT_JUDGE_DISPOSITION_REPAIR_EMPTY,
        DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED,
        DIRECT_JUDGE_DISPOSITION_FAIL_CLOSED_BLOCKED,
    ];

    #[test]
    fn direct_judge_outcome_contract_dispositions_are_frozen_and_total() {
        println!("{DIRECT_JUDGE_OUTCOME_SCHEMA}");
        for disposition in ALL_DISPOSITIONS {
            let outcome = DirectJudgeOutcomeV1::from_completion_facts(&facts(disposition))
                .expect("known disposition should project");
            assert_eq!(outcome.disposition, disposition);
            outcome
                .validate()
                .expect("projected receipt should validate");
        }
        assert!(disposition_family("direct_judge_unknown").is_err());
        assert!(
            DirectJudgeOutcomeV1::from_completion_facts(&facts("direct_judge_unknown")).is_err()
        );
    }

    #[test]
    fn direct_judge_outcome_contract_reward_integer_rule_is_monotone() {
        let passed =
            DirectJudgeOutcomeV1::from_completion_facts(&facts(DIRECT_JUDGE_DISPOSITION_PASSED))
                .expect("pass projects");
        assert_eq!(passed.reward_bps, Some(10_000));
        assert!(passed.reviewer_independent);

        let mut unverified = facts(DIRECT_JUDGE_DISPOSITION_RECHECK_PASSED);
        unverified.latest_mutation_verified = false;
        unverified.grounded_basis = "evidence_visible";
        let unverified =
            DirectJudgeOutcomeV1::from_completion_facts(&unverified).expect("projects");
        assert_eq!(unverified.reward_bps, Some(5_000));
        assert!(unverified.repair_round_used);

        let mut policy_not_required = unverified_facts(DIRECT_JUDGE_DISPOSITION_PASSED);
        policy_not_required.workspace_verification_required = false;
        let policy_not_required =
            DirectJudgeOutcomeV1::from_completion_facts(&policy_not_required).expect("projects");
        assert_eq!(policy_not_required.reward_bps, Some(10_000));

        let exhausted = DirectJudgeOutcomeV1::from_completion_facts(&facts(
            DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
        ))
        .expect("projects");
        assert_eq!(exhausted.reward_bps, Some(0));

        let blocked = DirectJudgeOutcomeV1::from_completion_facts(&facts(
            DIRECT_JUDGE_DISPOSITION_FAIL_CLOSED_BLOCKED,
        ))
        .expect("projects");
        assert_eq!(blocked.reward_bps, Some(0));
        assert_eq!(
            blocked.family,
            DirectJudgeDispositionFamilyV1::ReviseExhausted
        );
        assert!(blocked.reviewer_independent);
        assert!(blocked.repair_round_used);

        for disposition in [
            DIRECT_JUDGE_DISPOSITION_UNAVAILABLE,
            DIRECT_JUDGE_DISPOSITION_INCONCLUSIVE,
            DIRECT_JUDGE_DISPOSITION_RECHECK_INCONCLUSIVE,
            DIRECT_JUDGE_DISPOSITION_REPAIR_UNAVAILABLE,
            DIRECT_JUDGE_DISPOSITION_REPAIR_EMPTY,
            DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED,
        ] {
            let fail_open =
                DirectJudgeOutcomeV1::from_completion_facts(&facts(disposition)).expect("projects");
            assert_eq!(fail_open.reward_bps, Some(0));
            assert!(fail_open.family == DirectJudgeDispositionFamilyV1::FailOpen);
        }

        for disposition in [
            DIRECT_JUDGE_DISPOSITION_NOT_APPLICABLE,
            DIRECT_JUDGE_DISPOSITION_NOT_ELIGIBLE,
        ] {
            let censored =
                DirectJudgeOutcomeV1::from_completion_facts(&facts(disposition)).expect("projects");
            assert!(censored.censored());
            assert!(!censored.reviewer_independent);
        }
    }

    fn unverified_facts(disposition: &'static str) -> DirectJudgeCompletionFacts<'static> {
        let mut facts = facts(disposition);
        facts.latest_mutation_verified = false;
        facts
    }

    #[test]
    fn direct_judge_outcome_contract_receipt_digest_roundtrip_and_tamper() {
        let outcome =
            DirectJudgeOutcomeV1::from_completion_facts(&facts(DIRECT_JUDGE_DISPOSITION_PASSED))
                .expect("projects");
        let encoded = outcome.to_json().expect("encodes");
        let decoded = DirectJudgeOutcomeV1::from_json(&encoded).expect("decodes");
        assert_eq!(decoded, outcome);

        let mut tampered: serde_json::Value = serde_json::from_str(&encoded).expect("value");
        tampered["reward_bps"] = serde_json::json!(5000u64);
        let tampered_encoded = serde_json::to_string(&tampered).expect("encodes tamper");
        assert!(DirectJudgeOutcomeV1::from_json(&tampered_encoded).is_err());

        let mut tampered: serde_json::Value = serde_json::from_str(&encoded).expect("value");
        tampered["successful_mutations"] = serde_json::json!(0u64);
        let tampered_encoded = serde_json::to_string(&tampered).expect("encodes tamper");
        assert!(DirectJudgeOutcomeV1::from_json(&tampered_encoded).is_err());

        let mut drifted = outcome.clone();
        drifted.family = DirectJudgeDispositionFamilyV1::FailOpen;
        assert!(drifted.validate_payload().is_err());
    }

    #[test]
    fn mutation_state_consistency_is_enforced() {
        let mut zero_case = facts(DIRECT_JUDGE_DISPOSITION_PASSED);
        zero_case.successful_mutations = 0;
        zero_case.latest_mutation_verified = false;
        let outcome = DirectJudgeOutcomeV1::from_completion_facts(&zero_case).expect("projects");
        assert_eq!(
            outcome.mutation_verification,
            DirectJudgeMutationVerificationV1::NoMutations
        );
        assert_eq!(outcome.reward_bps, Some(10_000));

        let mut drifted = outcome.clone();
        drifted.successful_mutations = 3;
        assert!(drifted.validate_payload().is_err());
    }

    #[test]
    fn grounded_basis_is_whitelisted() {
        let mut bad = facts(DIRECT_JUDGE_DISPOSITION_PASSED);
        bad.grounded_basis = "claimed";
        assert!(DirectJudgeOutcomeV1::from_completion_facts(&bad).is_err());
    }

    #[test]
    fn fail_closed_block_only_for_judged_quality_failures_on_mutation_runs() {
        // Fail-open for infrastructure outcomes, disabled fail-closed, and
        // mutation-free runs.
        assert!(direct_judge_fail_closed_block(
            true,
            0,
            DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
            &[]
        )
        .is_none());
        assert!(direct_judge_fail_closed_block(
            false,
            3,
            DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
            &[]
        )
        .is_none());
        assert!(
            direct_judge_fail_closed_block(true, 3, DIRECT_JUDGE_DISPOSITION_UNAVAILABLE, &[])
                .is_none()
        );
        // Fail-closed for judged quality failures on mutation-bearing runs.
        let message = direct_judge_fail_closed_block(
            true,
            3,
            DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
            &["revise the claim".to_string()],
        )
        .expect("blocks delivery");
        assert!(message.contains("Delivery verification failed"));
        assert!(message.contains("revise the claim"));
    }
}
