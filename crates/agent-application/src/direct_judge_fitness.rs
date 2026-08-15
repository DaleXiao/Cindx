use crate::direct_judge_outcome::{
    DirectJudgeDispositionFamilyV1, DirectJudgeMutationVerificationV1, DirectJudgeOutcomeError,
    DirectJudgeOutcomeV1,
};
use serde::{Deserialize, Serialize};

pub const DIRECT_JUDGE_FITNESS_SIGNAL_SCHEMA: &str = "cindx.agent.direct-judge-fitness-signal.v1";
pub const DIRECT_JUDGE_FITNESS_SUMMARY_SCHEMA: &str = "cindx.agent.direct-judge-fitness-summary.v1";

pub const DIRECT_JUDGE_FITNESS_WINDOW: usize = 24;

/// Shadow fitness signal derived from one validated direct-judge outcome
/// receipt. Signals accumulate in an append-only shadow journal and are not
/// consumable by routing, prompt promotion, memory, canary, or serving until
/// an independent review admits them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectJudgeFitnessSignalV1 {
    pub schema: String,
    pub outcome_receipt_sha256: String,
    pub task_identity_sha256: String,
    pub family: DirectJudgeDispositionFamilyV1,
    pub reward_bps: Option<u16>,
    pub mutation_verification: DirectJudgeMutationVerificationV1,
    pub workspace_verification_required: bool,
    pub repair_round_used: bool,
    pub grounded_basis: String,
}

impl DirectJudgeFitnessSignalV1 {
    pub fn from_outcome(outcome: &DirectJudgeOutcomeV1) -> Result<Self, DirectJudgeOutcomeError> {
        outcome.validate()?;
        Ok(Self {
            schema: DIRECT_JUDGE_FITNESS_SIGNAL_SCHEMA.to_string(),
            outcome_receipt_sha256: outcome.receipt_sha256.clone(),
            task_identity_sha256: outcome.task_identity_sha256.clone(),
            family: outcome.family,
            reward_bps: outcome.reward_bps,
            mutation_verification: outcome.mutation_verification,
            workspace_verification_required: outcome.workspace_verification_required,
            repair_round_used: outcome.repair_round_used,
            grounded_basis: outcome.grounded_basis.clone(),
        })
    }

    pub fn from_json(encoded: &str) -> Result<Self, DirectJudgeOutcomeError> {
        let signal = serde_json::from_str::<Self>(encoded).map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge fitness signal JSON is invalid: {error}"
            ))
        })?;
        signal.validate()?;
        Ok(signal)
    }

    pub fn to_json(&self) -> Result<String, DirectJudgeOutcomeError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| {
            DirectJudgeOutcomeError::new(format!(
                "direct judge fitness signal JSON encoding failed: {error}"
            ))
        })
    }

    pub fn validate(&self) -> Result<(), DirectJudgeOutcomeError> {
        if self.schema != DIRECT_JUDGE_FITNESS_SIGNAL_SCHEMA {
            return Err(DirectJudgeOutcomeError::new(
                "direct judge fitness signal schema is unsupported",
            ));
        }
        validate_digest(&self.outcome_receipt_sha256, "outcome receipt")?;
        validate_digest(&self.task_identity_sha256, "task identity")?;
        if let Some(reward) = self.reward_bps {
            if reward > 10_000 {
                return Err(DirectJudgeOutcomeError::new(
                    "direct judge fitness signal reward exceeds its bound",
                ));
            }
        }
        match self.grounded_basis.as_str() {
            "postcondition_verified"
            | "evidence_visible"
            | "constraint_observed"
            | "self_contained" => {}
            other => {
                return Err(DirectJudgeOutcomeError::new(format!(
                    "direct judge fitness signal grounded basis is unsupported: {other}"
                )))
            }
        }
        Ok(())
    }

    pub fn censored(&self) -> bool {
        self.reward_bps.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectJudgeFitnessSummaryV1 {
    pub schema: String,
    pub window: usize,
    pub scored_runs: usize,
    pub censored_runs: usize,
    pub passed_runs: usize,
    pub revise_exhausted_runs: usize,
    pub fail_open_runs: usize,
    pub mutation_verified_rate_bps: Option<u16>,
    pub average_reward_bps: Option<u16>,
    pub promotion_eligible: bool,
}

/// Summarizes the most recent bounded window of deduplicated signals.
/// Censored (unjudged) signals are retained in the window count but stay out
/// of every reward denominator. The summary is never promotion-eligible; that
/// requires an independent review receipt outside this contract.
pub fn summarize_direct_judge_fitness(
    signals: &[DirectJudgeFitnessSignalV1],
) -> Result<DirectJudgeFitnessSummaryV1, DirectJudgeOutcomeError> {
    let mut seen = std::collections::BTreeSet::new();
    let mut window = Vec::new();
    for signal in signals.iter().rev() {
        signal.validate()?;
        if seen.insert(signal.outcome_receipt_sha256.clone()) {
            window.push(signal);
        }
    }
    window.reverse();
    if window.len() > DIRECT_JUDGE_FITNESS_WINDOW {
        window = window.split_off(window.len() - DIRECT_JUDGE_FITNESS_WINDOW);
    }
    let scored = window
        .iter()
        .filter(|signal| signal.reward_bps.is_some())
        .collect::<Vec<_>>();
    let censored_runs = window.len() - scored.len();
    let passed_runs = scored
        .iter()
        .filter(|signal| signal.family == DirectJudgeDispositionFamilyV1::Passed)
        .count();
    let revise_exhausted_runs = scored
        .iter()
        .filter(|signal| signal.family == DirectJudgeDispositionFamilyV1::ReviseExhausted)
        .count();
    let fail_open_runs = scored
        .iter()
        .filter(|signal| signal.family == DirectJudgeDispositionFamilyV1::FailOpen)
        .count();
    let mutated = scored
        .iter()
        .filter(|signal| {
            signal.mutation_verification != DirectJudgeMutationVerificationV1::NoMutations
        })
        .count();
    let mutated_verified = scored
        .iter()
        .filter(|signal| {
            signal.mutation_verification
                == DirectJudgeMutationVerificationV1::VerifiedAfterLastMutation
        })
        .count();
    let mutation_verified_rate_bps = std::num::NonZeroUsize::new(mutated).map(|denominator| {
        u16::try_from(mutated_verified * 10_000 / denominator.get()).unwrap_or(10_000)
    });
    let average_reward_bps = if scored.is_empty() {
        None
    } else {
        let total = scored
            .iter()
            .map(|signal| u64::from(signal.reward_bps.unwrap_or_default()))
            .sum::<u64>();
        Some(u16::try_from(total / scored.len() as u64).unwrap_or(10_000))
    };
    Ok(DirectJudgeFitnessSummaryV1 {
        schema: DIRECT_JUDGE_FITNESS_SUMMARY_SCHEMA.to_string(),
        window: window.len(),
        scored_runs: scored.len(),
        censored_runs,
        passed_runs,
        revise_exhausted_runs,
        fail_open_runs,
        mutation_verified_rate_bps,
        average_reward_bps,
        promotion_eligible: false,
    })
}

fn validate_digest(value: &str, label: &str) -> Result<(), DirectJudgeOutcomeError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DirectJudgeOutcomeError::new(format!(
            "direct judge fitness signal {label} is not a SHA-256 digest"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct_judge_outcome::DirectJudgeCompletionFacts;

    fn outcome(disposition: &'static str, verified: bool) -> DirectJudgeOutcomeV1 {
        DirectJudgeOutcomeV1::from_completion_facts(&DirectJudgeCompletionFacts {
            task_identity: "task-shadow",
            disposition,
            successful_mutations: 1,
            latest_mutation_verified: verified,
            workspace_verification_required: true,
            grounded_basis: "postcondition_verified",
        })
        .expect("outcome projects")
    }

    fn signal(disposition: &'static str, verified: bool) -> DirectJudgeFitnessSignalV1 {
        DirectJudgeFitnessSignalV1::from_outcome(&outcome(disposition, verified)).expect("signal")
    }

    #[test]
    fn signal_binds_the_validated_outcome_receipt() {
        let full = outcome("direct_judge_passed", true);
        let signal = DirectJudgeFitnessSignalV1::from_outcome(&full).expect("binds");
        assert_eq!(signal.outcome_receipt_sha256, full.receipt_sha256);
        assert_eq!(signal.reward_bps, Some(10_000));
        let decoded = DirectJudgeFitnessSignalV1::from_json(&signal.to_json().expect("encodes"))
            .expect("roundtrip");
        assert_eq!(decoded, signal);

        let mut tampered = full.clone();
        tampered.receipt_sha256 = "0".repeat(64);
        assert!(DirectJudgeFitnessSignalV1::from_outcome(&tampered).is_err());
    }

    #[test]
    fn direct_judge_outcome_contract_fitness_keeps_censored_runs_out_of_denominators() {
        let signals = vec![
            signal("direct_judge_passed", true),
            signal("direct_judge_passed", false),
            signal("direct_judge_recheck_exhausted", true),
            signal("direct_judge_unavailable", true),
            signal("direct_judge_not_applicable", true),
            signal("direct_judge_not_eligible", true),
        ];
        let summary = summarize_direct_judge_fitness(&signals).expect("summarizes");
        assert_eq!(summary.window, 6);
        assert_eq!(summary.scored_runs, 4);
        assert_eq!(summary.censored_runs, 2);
        assert_eq!(summary.passed_runs, 2);
        assert_eq!(summary.revise_exhausted_runs, 1);
        assert_eq!(summary.fail_open_runs, 1);
        assert_eq!(
            summary.average_reward_bps,
            Some(15_000 / 4)
        );
        assert_eq!(summary.mutation_verified_rate_bps, Some(3 * 10_000 / 4));
        assert!(!summary.promotion_eligible);
    }

    #[test]
    fn direct_judge_outcome_contract_fitness_window_deduplicates_and_bounds() {
        let unique = signal("direct_judge_passed", true);
        let mut signals = vec![unique.clone(), unique.clone(), unique.clone()];
        let summary = summarize_direct_judge_fitness(&signals).expect("summarizes");
        assert_eq!(summary.window, 1);

        for index in 0..(DIRECT_JUDGE_FITNESS_WINDOW + 5) {
            signals.push(
                DirectJudgeFitnessSignalV1::from_outcome(
                    &DirectJudgeOutcomeV1::from_completion_facts(&DirectJudgeCompletionFacts {
                        task_identity: &format!("task-{index}"),
                        disposition: "direct_judge_passed",
                        successful_mutations: 0,
                        latest_mutation_verified: false,
                        workspace_verification_required: false,
                        grounded_basis: "self_contained",
                    })
                    .expect("outcome"),
                )
                .expect("signal"),
            );
        }
        let summary = summarize_direct_judge_fitness(&signals).expect("summarizes");
        assert_eq!(summary.window, DIRECT_JUDGE_FITNESS_WINDOW);
        assert!(!summary.promotion_eligible);
    }

    #[test]
    fn summary_fails_closed_on_invalid_signals() {
        let valid = signal("direct_judge_passed", true);
        let mut drifted = valid.clone();
        drifted.schema = "cindx.unknown.v9".to_string();
        assert!(summarize_direct_judge_fitness(&[valid.clone(), drifted]).is_err());

        let mut drifted = valid.clone();
        drifted.grounded_basis = "claimed".to_string();
        assert!(summarize_direct_judge_fitness(&[valid.clone(), drifted]).is_err());

        let mut drifted = valid.clone();
        drifted.reward_bps = Some(20_000);
        assert!(summarize_direct_judge_fitness(&[valid, drifted]).is_err());
    }

    #[test]
    fn direct_judge_outcome_contract_fitness_channel_stays_shadow_only() {
        println!("{DIRECT_JUDGE_FITNESS_SUMMARY_SCHEMA}");
        let signals = vec![
            signal("direct_judge_passed", true),
            signal("direct_judge_recheck_exhausted", true),
            signal("direct_judge_unavailable", true),
            signal("direct_judge_not_applicable", true),
        ];
        let summary = summarize_direct_judge_fitness(&signals).expect("summarizes");
        assert!(!summary.promotion_eligible);
        let empty = summarize_direct_judge_fitness(&[]).expect("summarizes empty");
        assert!(!empty.promotion_eligible);
        assert_eq!(empty.window, 0);
        assert_eq!(empty.average_reward_bps, None);
    }
}
