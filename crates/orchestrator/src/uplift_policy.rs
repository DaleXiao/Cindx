use crate::AnytimeCandidateKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpliftGap {
    MissingDeliverable,
    Unsafe,
    Unverified,
    MissingSynthesis,
    InsufficientDistinctContributions,
    MissingAnchorComparison,
    InsufficientUplift,
}

impl UpliftGap {
    pub fn label(self) -> &'static str {
        match self {
            Self::MissingDeliverable => "missing_deliverable",
            Self::Unsafe => "unsafe",
            Self::Unverified => "unverified",
            Self::MissingSynthesis => "missing_synthesis",
            Self::InsufficientDistinctContributions => "insufficient_distinct_contributions",
            Self::MissingAnchorComparison => "missing_anchor_comparison",
            Self::InsufficientUplift => "insufficient_uplift",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpliftGateInput {
    pub team_candidate_kind: AnytimeCandidateKind,
    pub team_deliverable: bool,
    pub team_verified: bool,
    pub team_safety_violations: u64,
    pub distinct_contributions: usize,
    pub required_distinct_contributions: usize,
    pub requires_synthesis: bool,
    pub requires_anchor_comparison: bool,
    pub paired_uplift_bps: Option<i16>,
    pub required_uplift_bps: u16,
    pub repair_attempts_remaining: usize,
    pub anchor_eligible: bool,
}

impl UpliftGateInput {
    pub fn gaps(&self) -> Vec<UpliftGap> {
        let mut gaps = Vec::new();
        if !self.team_deliverable {
            gaps.push(UpliftGap::MissingDeliverable);
        }
        if self.team_safety_violations > 0 {
            gaps.push(UpliftGap::Unsafe);
        }
        if !self.team_verified {
            gaps.push(UpliftGap::Unverified);
        }
        if self.requires_synthesis && self.team_candidate_kind != AnytimeCandidateKind::Synthesis {
            gaps.push(UpliftGap::MissingSynthesis);
        }
        if self.distinct_contributions < self.required_distinct_contributions {
            gaps.push(UpliftGap::InsufficientDistinctContributions);
        }
        if self.requires_anchor_comparison {
            match self.paired_uplift_bps {
                Some(uplift) if uplift >= self.required_uplift_bps as i16 => {}
                Some(_) => gaps.push(UpliftGap::InsufficientUplift),
                None => gaps.push(UpliftGap::MissingAnchorComparison),
            }
        }
        gaps
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpliftGateDecision {
    AcceptTeam,
    RepairTeam { gaps: Vec<UpliftGap> },
    SelectAnchor { gaps: Vec<UpliftGap> },
    ReturnBestKnown { gaps: Vec<UpliftGap> },
}

impl UpliftGateDecision {
    pub fn gaps(&self) -> &[UpliftGap] {
        match self {
            Self::AcceptTeam => &[],
            Self::RepairTeam { gaps }
            | Self::SelectAnchor { gaps }
            | Self::ReturnBestKnown { gaps } => gaps,
        }
    }
}

pub fn decide_uplift_gate(input: &UpliftGateInput) -> UpliftGateDecision {
    let gaps = input.gaps();
    if gaps.is_empty() {
        return UpliftGateDecision::AcceptTeam;
    }
    if input.repair_attempts_remaining > 0 {
        return UpliftGateDecision::RepairTeam { gaps };
    }
    if input.anchor_eligible {
        return UpliftGateDecision::SelectAnchor { gaps };
    }
    UpliftGateDecision::ReturnBestKnown { gaps }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verified_synthesis() -> UpliftGateInput {
        UpliftGateInput {
            team_candidate_kind: AnytimeCandidateKind::Synthesis,
            team_deliverable: true,
            team_verified: true,
            team_safety_violations: 0,
            distinct_contributions: 2,
            required_distinct_contributions: 2,
            requires_synthesis: true,
            requires_anchor_comparison: true,
            paired_uplift_bps: Some(300),
            required_uplift_bps: 250,
            repair_attempts_remaining: 1,
            anchor_eligible: true,
        }
    }

    #[test]
    fn accepts_only_a_complete_verified_uplift() {
        assert_eq!(
            decide_uplift_gate(&verified_synthesis()),
            UpliftGateDecision::AcceptTeam
        );
    }

    #[test]
    fn underperforming_team_gets_one_targeted_repair() {
        let mut input = verified_synthesis();
        input.paired_uplift_bps = Some(-500);
        assert_eq!(
            decide_uplift_gate(&input),
            UpliftGateDecision::RepairTeam {
                gaps: vec![UpliftGap::InsufficientUplift]
            }
        );
    }

    #[test]
    fn exhausted_repair_selects_a_safe_anchor() {
        let mut input = verified_synthesis();
        input.team_verified = false;
        input.repair_attempts_remaining = 0;
        assert_eq!(
            decide_uplift_gate(&input),
            UpliftGateDecision::SelectAnchor {
                gaps: vec![UpliftGap::Unverified]
            }
        );
    }

    #[test]
    fn unsafe_team_is_never_accepted() {
        let mut input = verified_synthesis();
        input.team_safety_violations = 1;
        input.repair_attempts_remaining = 0;
        input.anchor_eligible = false;
        assert_eq!(
            decide_uplift_gate(&input),
            UpliftGateDecision::ReturnBestKnown {
                gaps: vec![UpliftGap::Unsafe]
            }
        );
    }

    #[test]
    fn non_collaborative_contract_does_not_require_anchor_uplift() {
        let mut input = verified_synthesis();
        input.requires_synthesis = false;
        input.requires_anchor_comparison = false;
        input.required_distinct_contributions = 0;
        input.paired_uplift_bps = None;
        assert_eq!(decide_uplift_gate(&input), UpliftGateDecision::AcceptTeam);
    }
}
