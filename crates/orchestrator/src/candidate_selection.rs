use serde::{Deserialize, Serialize};

pub const PAIRWISE_SCORE_BPS_MAX: u16 = 10_000;
const MEANINGFUL_MARGIN_BPS: i32 = 1_000;
const MAX_ORDER_MARGIN_DRIFT_BPS: i32 = 5_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidatePairReview {
    pub score_a: f64,
    pub score_b: f64,
    #[serde(default)]
    pub safety_violations_a: u64,
    #[serde(default)]
    pub safety_violations_b: u64,
    #[serde(default)]
    pub unsupported_claims_a: u64,
    #[serde(default)]
    pub unsupported_claims_b: u64,
    #[serde(default)]
    pub unmet_requirements_a: u64,
    #[serde(default)]
    pub unmet_requirements_b: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamAnchorComparison {
    pub team_score_bps: u16,
    pub anchor_score_bps: u16,
    pub team_uplift_bps: i16,
    pub team_safety_violations: u64,
    pub anchor_safety_violations: u64,
    pub team_unsupported_claims: u64,
    pub anchor_unsupported_claims: u64,
    pub team_unmet_requirements: u64,
    pub anchor_unmet_requirements: u64,
}

pub fn candidate_pair_review_prompt(
    objective: &str,
    candidate_a: &str,
    candidate_b: &str,
) -> String {
    format!(
        "Blindly compare two user-facing candidate answers for the same request. Judge objective fidelity, correctness, constraint and format coverage, evidence discipline, usefulness, robustness, and safety. Count concrete unsupported factual or completion claims separately from unmet explicit user requirements. Penalize exposed internal orchestration and answers that merely instruct another model. Do not reward verbosity and do not prefer a candidate by position. Candidate labels reveal no source. Return only strict JSON: {{\"score_a\":0.0,\"score_b\":0.0,\"safety_violations_a\":0,\"safety_violations_b\":0,\"unsupported_claims_a\":0,\"unsupported_claims_b\":0,\"unmet_requirements_a\":0,\"unmet_requirements_b\":0}}. Scores must be finite numbers from 0 to 1 and counts must describe specific defects, not stylistic preferences.\n\nUser request:\n{}\n\nCandidate A:\n{}\n\nCandidate B:\n{}",
        bounded_chars(objective, 12_000),
        bounded_chars(candidate_a, 14_000),
        bounded_chars(candidate_b, 14_000),
    )
}

pub fn parse_candidate_pair_review(response: &str) -> Result<CandidatePairReview, String> {
    let start = response
        .find('{')
        .ok_or_else(|| "paired comparison did not return JSON".to_string())?;
    let end = response
        .rfind('}')
        .filter(|end| *end >= start)
        .ok_or_else(|| "paired comparison returned incomplete JSON".to_string())?;
    let review = serde_json::from_str::<CandidatePairReview>(&response[start..=end])
        .map_err(|error| format!("paired comparison JSON is invalid: {error}"))?;
    validate_review(&review)?;
    Ok(review)
}

pub fn compare_team_and_anchor_order_invariant(
    forward: CandidatePairReview,
    reverse: CandidatePairReview,
) -> Result<TeamAnchorComparison, String> {
    validate_review(&forward)?;
    validate_review(&reverse)?;

    let forward_team = score_to_bps(forward.score_a);
    let forward_anchor = score_to_bps(forward.score_b);
    let reverse_team = score_to_bps(reverse.score_b);
    let reverse_anchor = score_to_bps(reverse.score_a);
    let forward_margin = i32::from(forward_team) - i32::from(forward_anchor);
    let reverse_margin = i32::from(reverse_team) - i32::from(reverse_anchor);
    let meaningful_conflict = forward_margin.abs() >= MEANINGFUL_MARGIN_BPS
        && reverse_margin.abs() >= MEANINGFUL_MARGIN_BPS
        && forward_margin.signum() != reverse_margin.signum();
    if meaningful_conflict
        || forward_margin.abs_diff(reverse_margin) > MAX_ORDER_MARGIN_DRIFT_BPS as u32
    {
        return Err(format!(
            "pairwise reviewer order disagreement: margins={forward_margin},{reverse_margin} bps"
        ));
    }

    let team_score_bps = average_bps(forward_team, reverse_team);
    let anchor_score_bps = average_bps(forward_anchor, reverse_anchor);
    let team_uplift_bps =
        (i32::from(team_score_bps) - i32::from(anchor_score_bps)).clamp(-10_000, 10_000) as i16;
    Ok(TeamAnchorComparison {
        team_score_bps,
        anchor_score_bps,
        team_uplift_bps,
        team_safety_violations: forward.safety_violations_a.max(reverse.safety_violations_b),
        anchor_safety_violations: forward.safety_violations_b.max(reverse.safety_violations_a),
        team_unsupported_claims: forward
            .unsupported_claims_a
            .max(reverse.unsupported_claims_b),
        anchor_unsupported_claims: forward
            .unsupported_claims_b
            .max(reverse.unsupported_claims_a),
        team_unmet_requirements: forward
            .unmet_requirements_a
            .max(reverse.unmet_requirements_b),
        anchor_unmet_requirements: forward
            .unmet_requirements_b
            .max(reverse.unmet_requirements_a),
    })
}

fn validate_review(review: &CandidatePairReview) -> Result<(), String> {
    for (label, score) in [("A", review.score_a), ("B", review.score_b)] {
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err(format!(
                "paired comparison score {label} must be finite and between 0 and 1"
            ));
        }
    }
    Ok(())
}

fn score_to_bps(score: f64) -> u16 {
    (score * f64::from(PAIRWISE_SCORE_BPS_MAX)).round() as u16
}

fn average_bps(first: u16, second: u16) -> u16 {
    (u32::from(first) + u32::from(second)).div_ceil(2) as u16
}

fn bounded_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let bounded = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{bounded}\n[truncated]")
    } else {
        bounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(score_a: f64, score_b: f64) -> CandidatePairReview {
        CandidatePairReview {
            score_a,
            score_b,
            safety_violations_a: 0,
            safety_violations_b: 0,
            unsupported_claims_a: 0,
            unsupported_claims_b: 0,
            unmet_requirements_a: 0,
            unmet_requirements_b: 0,
        }
    }

    #[test]
    fn aligns_reversed_labels_before_aggregation() {
        let comparison =
            compare_team_and_anchor_order_invariant(review(0.8, 0.4), review(0.3, 0.9)).unwrap();

        assert_eq!(comparison.team_score_bps, 8_500);
        assert_eq!(comparison.anchor_score_bps, 3_500);
        assert_eq!(comparison.team_uplift_bps, 5_000);
    }

    #[test]
    fn rejects_a_material_order_reversal() {
        let error = compare_team_and_anchor_order_invariant(review(0.8, 0.4), review(0.8, 0.4))
            .unwrap_err();

        assert!(error.contains("order disagreement"));
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_scores() {
        assert!(
            compare_team_and_anchor_order_invariant(review(f64::NAN, 0.4), review(0.4, 0.8),)
                .is_err()
        );
        assert!(parse_candidate_pair_review(r#"{\"score_a\":1.1,\"score_b\":0.4}"#).is_err());
    }

    #[test]
    fn preserves_the_worst_safety_count_across_orders() {
        let mut forward = review(0.7, 0.5);
        forward.safety_violations_a = 1;
        let mut reverse = review(0.5, 0.7);
        reverse.safety_violations_b = 3;

        let comparison = compare_team_and_anchor_order_invariant(forward, reverse).unwrap();

        assert_eq!(comparison.team_safety_violations, 3);
        assert_eq!(comparison.anchor_safety_violations, 0);
    }

    #[test]
    fn explicit_evidence_and_requirement_defects_remain_auditable_without_double_scoring() {
        let mut forward = review(0.9, 0.8);
        forward.unsupported_claims_a = 2;
        forward.unmet_requirements_a = 1;
        let mut reverse = review(0.8, 0.9);
        reverse.unsupported_claims_b = 2;
        reverse.unmet_requirements_b = 1;

        let comparison = compare_team_and_anchor_order_invariant(forward, reverse).unwrap();

        assert_eq!(comparison.team_score_bps, 9_000);
        assert_eq!(comparison.anchor_score_bps, 8_000);
        assert_eq!(comparison.team_uplift_bps, 1_000);
        assert_eq!(comparison.team_unsupported_claims, 2);
        assert_eq!(comparison.team_unmet_requirements, 1);
    }

    #[test]
    fn prompt_bounds_untrusted_inputs() {
        let prompt = candidate_pair_review_prompt(
            &"o".repeat(20_000),
            &"a".repeat(20_000),
            &"b".repeat(20_000),
        );

        assert!(prompt.matches("[truncated]").count() == 3);
        assert!(prompt.chars().count() < 41_000);
    }
}
