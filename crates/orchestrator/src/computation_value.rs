use crate::{AgentExecutionMode, AgentRunDecision};
use agent_core::Metadata;

pub const AGENT_ROUTE_OBSERVABILITY_KEYS: [&str; 9] = [
    "decision_calibration",
    "candidate_route_tier",
    "selected_route_tier",
    "computation_verdict",
    "computation_benefit_bps",
    "computation_required_value_bps",
    "computation_net_value_bps",
    "computation_units",
    "computation_matched_examples",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentDecisionCalibration {
    MatchedEvidence,
    ValueOfComputation,
}

impl AgentDecisionCalibration {
    pub const fn label(self) -> &'static str {
        match self {
            Self::MatchedEvidence => "matched_evidence_direct",
            Self::ValueOfComputation => "value_of_computation_direct",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRouteTier {
    Direct,
    GroundedDirect,
    Workflow,
}

impl AgentRouteTier {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::GroundedDirect => "grounded_direct",
            Self::Workflow => "workflow",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoComputationVerdict {
    Admit,
    BelowPredictionFloor,
    NoIndependentContributions,
    SerialInteraction,
    NegativeExpectedValue,
}

impl AutoComputationVerdict {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Admit => "admit",
            Self::BelowPredictionFloor => "below_prediction_floor",
            Self::NoIndependentContributions => "no_independent_contributions",
            Self::SerialInteraction => "serial_interaction",
            Self::NegativeExpectedValue => "negative_expected_value",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoComputationAssessment {
    pub verdict: AutoComputationVerdict,
    pub confidence_weighted_benefit_bps: i32,
    pub evidence_adjusted_benefit_bps: i32,
    pub required_value_bps: i32,
    pub net_value_bps: i32,
    pub estimated_compute_units: usize,
    pub matched_examples: usize,
}

impl AutoComputationAssessment {
    pub fn admitted(&self) -> bool {
        self.verdict == AutoComputationVerdict::Admit
    }

    pub fn rationale(&self) -> String {
        format!(
            "Auto value-of-computation {}: adjusted_benefit={}bps required_value={}bps net_value={}bps compute_units={} matched_examples={}",
            self.verdict.label(),
            self.evidence_adjusted_benefit_bps,
            self.required_value_bps,
            self.net_value_bps,
            self.estimated_compute_units,
            self.matched_examples,
        )
    }

    pub fn from_causal_route(receipt: &crate::CausalRouteSelectionV2) -> Self {
        let verdict = match receipt.reason {
            crate::CausalRouteReason::AdmitPositiveValue
            | crate::CausalRouteReason::CandidateDirect
            | crate::CausalRouteReason::FastPolicy => AutoComputationVerdict::Admit,
            crate::CausalRouteReason::BelowPredictionFloor => {
                AutoComputationVerdict::BelowPredictionFloor
            }
            crate::CausalRouteReason::NoIndependentDemand => {
                AutoComputationVerdict::NoIndependentContributions
            }
            crate::CausalRouteReason::SerialInteraction => {
                AutoComputationVerdict::SerialInteraction
            }
            crate::CausalRouteReason::MatchedEvidenceAgainst
            | crate::CausalRouteReason::NegativeExpectedValue
            | crate::CausalRouteReason::NoCompatibleRoute
            | crate::CausalRouteReason::ExecutionConstraint
            | crate::CausalRouteReason::DegradedFallback => {
                AutoComputationVerdict::NegativeExpectedValue
            }
        };
        Self {
            verdict,
            confidence_weighted_benefit_bps: receipt.predicted_benefit_bps,
            evidence_adjusted_benefit_bps: receipt.evidence_adjusted_benefit_bps,
            required_value_bps: receipt
                .coordination_cost_bps
                .saturating_add(receipt.latency_cost_bps)
                .saturating_add(receipt.uncertainty_bps),
            net_value_bps: receipt.net_value_lower_bps,
            estimated_compute_units: receipt
                .actions
                .first()
                .map(|action| usize::from(action.projected_model_calls))
                .unwrap_or(1),
            matched_examples: receipt.support.matched_examples,
        }
    }
}

impl AgentRunDecision {
    pub fn route_tier(&self) -> AgentRouteTier {
        match self.execution {
            AgentExecutionMode::Workflow => AgentRouteTier::Workflow,
            AgentExecutionMode::Direct if self.retrieval.enabled() || self.memory.enabled() => {
                AgentRouteTier::GroundedDirect
            }
            AgentExecutionMode::Direct => AgentRouteTier::Direct,
        }
    }

    pub fn candidate_route_tier(&self) -> AgentRouteTier {
        if let Some(receipt) = &self.causal_route {
            receipt.candidate_route
        } else if self.calibration.is_some() || self.computation_value.is_some() {
            AgentRouteTier::Workflow
        } else {
            self.route_tier()
        }
    }

    pub fn route_observability_metadata(&self) -> Metadata {
        let mut metadata = [
            (
                "candidate_route_tier".to_string(),
                self.candidate_route_tier().label().to_string(),
            ),
            (
                "selected_route_tier".to_string(),
                self.route_tier().label().to_string(),
            ),
        ]
        .into_iter()
        .collect::<Metadata>();
        if let Some(calibration) = self.calibration {
            metadata.insert(
                "decision_calibration".to_string(),
                calibration.label().to_string(),
            );
        }
        if let Some(value) = &self.computation_value {
            for (key, value) in [
                ("computation_verdict", value.verdict.label().to_string()),
                (
                    "computation_benefit_bps",
                    value.evidence_adjusted_benefit_bps.to_string(),
                ),
                (
                    "computation_required_value_bps",
                    value.required_value_bps.to_string(),
                ),
                ("computation_net_value_bps", value.net_value_bps.to_string()),
                (
                    "computation_units",
                    value.estimated_compute_units.to_string(),
                ),
                (
                    "computation_matched_examples",
                    value.matched_examples.to_string(),
                ),
            ] {
                metadata.insert(key.to_string(), value);
            }
        }
        metadata
    }
}
