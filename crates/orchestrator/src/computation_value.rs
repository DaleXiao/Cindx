use crate::{
    AgentExecutionMode, AgentRiskLevel, AgentRunDecision, AgentToolRequirement,
    AgentVerificationPolicy, MatchedCollaborationEvidence, MatchedCollaborationEvidenceTeacher,
    RoutingContext, TaskClass, AUTO_COLLABORATION_MIN_CONFIDENCE_BPS,
    AUTO_COLLABORATION_MIN_UPLIFT_BPS,
};
use agent_core::Metadata;

const AUTO_VALUE_PRIOR_SAMPLES: usize = 4;
const AUTO_MAX_MATCHED_SAMPLES: usize = 32;
const AUTO_WEIGHTED_ADMISSION_FLOOR_BPS: i32 = AUTO_COLLABORATION_MIN_UPLIFT_BPS as i32
    * AUTO_COLLABORATION_MIN_CONFIDENCE_BPS as i32
    / 10_000;
const AUTO_MIN_VALUE_PER_COMPUTE_UNIT_BPS: i32 = (AUTO_WEIGHTED_ADMISSION_FLOOR_BPS + 1) / 2;

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

pub fn matching_collaboration_evidence<'a>(
    decision: &AgentRunDecision,
    effort: &str,
    evidence: &'a [MatchedCollaborationEvidence],
) -> Option<&'a MatchedCollaborationEvidence> {
    let signature = decision.learning_signature();
    evidence.iter().find(|entry| {
        entry.task_class == decision.task_class
            && entry.effort.eq_ignore_ascii_case(effort)
            && entry.routing_signature == signature
    })
}

pub fn matching_collaboration_evidence_for_context<'a>(
    decision: &AgentRunDecision,
    effort: &str,
    context_fingerprint: &str,
    route_action_id: &str,
    evidence: &'a MatchedCollaborationEvidenceTeacher,
) -> (Option<&'a MatchedCollaborationEvidence>, usize) {
    evidence.match_context_action(
        &decision.task_class,
        effort,
        context_fingerprint,
        route_action_id,
        &decision.learning_signature(),
    )
}

pub fn assess_auto_computation_value(
    decision: &AgentRunDecision,
    objective_context: &RoutingContext,
    matched_evidence: Option<&MatchedCollaborationEvidence>,
) -> AutoComputationAssessment {
    debug_assert_eq!(decision.execution, AgentExecutionMode::Workflow);

    let confidence_weighted_benefit_bps = i32::from(decision.expected_uplift_bps)
        .saturating_mul(i32::from(decision.confidence_bps))
        / 10_000;
    let independent_contributions =
        decision.max_parallelism >= 2 && decision.distinct_contributions >= 2;
    let serial_interaction = matches!(
        decision.task_class,
        TaskClass::Browser | TaskClass::Computer
    );
    let effectful_execution = decision.tool_requirement == AgentToolRequirement::Effects;
    let serial_exception = objective_context.needs_multi_model
        || (decision.risk_level == AgentRiskLevel::High
            && decision.verification == AgentVerificationPolicy::Independent);

    let mut estimated_compute_units = decision
        .distinct_contributions
        .saturating_add(1)
        .saturating_add(usize::from(
            decision.verification == AgentVerificationPolicy::Independent,
        ))
        .max(decision.estimated_steps);
    if serial_interaction || effectful_execution {
        estimated_compute_units = estimated_compute_units.saturating_add(1);
    }

    let (evidence_adjusted_benefit_bps, matched_examples, latency_penalty) = matched_evidence
        .filter(|evidence| evidence.evidence_ready())
        .map(|evidence| {
            let samples = evidence.examples.min(AUTO_MAX_MATCHED_SAMPLES);
            let blended = confidence_weighted_benefit_bps
                .saturating_mul(AUTO_VALUE_PRIOR_SAMPLES as i32)
                .saturating_add(
                    i32::from(evidence.average_uplift_bps).saturating_mul(samples as i32),
                )
                / (AUTO_VALUE_PRIOR_SAMPLES + samples) as i32;
            let latency_penalty = evidence
                .average_anchor_latency_ms
                .filter(|latency| *latency > 0)
                .is_some_and(|anchor| evidence.average_team_latency_ms > anchor.saturating_mul(2));
            (blended, evidence.examples, latency_penalty)
        })
        .unwrap_or((confidence_weighted_benefit_bps, 0, false));
    if latency_penalty {
        estimated_compute_units = estimated_compute_units.saturating_add(1);
    }

    let required_value_bps =
        (estimated_compute_units as i32).saturating_mul(AUTO_MIN_VALUE_PER_COMPUTE_UNIT_BPS);
    let net_value_bps = evidence_adjusted_benefit_bps.saturating_sub(required_value_bps);
    let prediction_floor_met = decision.expected_uplift_bps >= AUTO_COLLABORATION_MIN_UPLIFT_BPS
        && decision.confidence_bps >= AUTO_COLLABORATION_MIN_CONFIDENCE_BPS;
    let verdict = if !prediction_floor_met {
        AutoComputationVerdict::BelowPredictionFloor
    } else if !independent_contributions {
        AutoComputationVerdict::NoIndependentContributions
    } else if serial_interaction && !serial_exception {
        AutoComputationVerdict::SerialInteraction
    } else if net_value_bps < 0 {
        AutoComputationVerdict::NegativeExpectedValue
    } else {
        AutoComputationVerdict::Admit
    };

    AutoComputationAssessment {
        verdict,
        confidence_weighted_benefit_bps,
        evidence_adjusted_benefit_bps,
        required_value_bps,
        net_value_bps,
        estimated_compute_units,
        matched_examples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentRiskLevel, MemoryRecallPlan, MemoryRecallPolicy, ModelCandidate,
        WorkspaceRetrievalChannel,
    };
    use agent_core::ModelRole;
    use std::collections::BTreeSet;

    fn context(prompt: &str) -> RoutingContext {
        RoutingContext::from_prompt(
            prompt,
            vec![ModelCandidate {
                name: "executor".to_string(),
                role: ModelRole::Executor,
                supports_tools: true,
                supports_vision: true,
                tools_capability_source: crate::ModelCapabilitySource::Configured,
                vision_capability_source: crate::ModelCapabilitySource::Configured,
                cost_tier: 1,
                latency_tier: 1,
            }],
        )
    }

    fn workflow() -> AgentRunDecision {
        let mut decision = AgentRunDecision::direct("executor");
        decision.task_class = TaskClass::Research;
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::SelfCheck;
        decision.max_parallelism = 2;
        decision.min_successful_branches = 2;
        decision.distinct_contributions = 2;
        decision.estimated_steps = 3;
        decision.expected_uplift_bps = 3_500;
        decision.confidence_bps = 7_500;
        decision.stop_policy = crate::ConductorStopPolicy::Quorum;
        decision
    }

    #[test]
    fn admits_only_positive_value_independent_work() {
        let decision = workflow();
        let admitted = assess_auto_computation_value(
            &decision,
            &context("Compare two independent implementation strategies"),
            None,
        );
        assert!(admitted.admitted());
        assert!(admitted.net_value_bps >= 0);

        let mut weak = decision;
        weak.expected_uplift_bps = AUTO_COLLABORATION_MIN_UPLIFT_BPS;
        weak.confidence_bps = AUTO_COLLABORATION_MIN_CONFIDENCE_BPS;
        let rejected = assess_auto_computation_value(
            &weak,
            &context("Compare two independent implementation strategies"),
            None,
        );
        assert_eq!(
            rejected.verdict,
            AutoComputationVerdict::NegativeExpectedValue
        );
    }

    #[test]
    fn semantic_independence_does_not_depend_on_prompt_keywords() {
        let mut decision = workflow();
        decision.expected_uplift_bps = 5_000;
        decision.confidence_bps = 8_000;
        let objective = context("分别分析缓存一致性与故障恢复，最后综合建议");
        assert!(!objective.parallelizable);
        assert!(assess_auto_computation_value(&decision, &objective, None).admitted());
    }

    #[test]
    fn grounded_direct_is_distinct_from_plain_direct() {
        let mut decision = AgentRunDecision::direct("executor");
        assert_eq!(decision.route_tier(), AgentRouteTier::Direct);
        decision.retrieval.channels = [WorkspaceRetrievalChannel::Semantic]
            .into_iter()
            .collect::<BTreeSet<_>>();
        decision.retrieval.query = "project policy".to_string();
        decision.memory = MemoryRecallPlan {
            policy: MemoryRecallPolicy::Relevant,
            query: "prior user decision".to_string(),
        };
        assert_eq!(decision.route_tier(), AgentRouteTier::GroundedDirect);
    }

    #[test]
    fn interactive_browser_work_requires_an_explicit_or_high_risk_parallel_exception() {
        let mut decision = workflow();
        decision.task_class = TaskClass::Browser;
        decision.tool_requirement = AgentToolRequirement::Effects;
        let ordinary = assess_auto_computation_value(
            &decision,
            &context("Open the website and capture the incident evidence"),
            None,
        );
        assert_eq!(ordinary.verdict, AutoComputationVerdict::SerialInteraction);

        decision.risk_level = AgentRiskLevel::High;
        decision.verification = AgentVerificationPolicy::Independent;
        decision.expected_uplift_bps = 8_000;
        decision.confidence_bps = 9_000;
        let explicit = assess_auto_computation_value(
            &decision,
            &context("Open the production site and verify the incident evidence"),
            None,
        );
        assert!(explicit.admitted());
    }

    #[test]
    fn effectful_coding_pays_extra_cost_without_disabling_independent_analysis() {
        let mut decision = workflow();
        decision.task_class = TaskClass::Coding;
        decision.tool_requirement = AgentToolRequirement::Effects;
        decision.expected_uplift_bps = 6_000;
        decision.confidence_bps = 8_000;
        let assessment = assess_auto_computation_value(
            &decision,
            &context("分别分析状态迁移和恢复边界，再由前台执行修改"),
            None,
        );
        assert!(assessment.admitted());
        assert_eq!(assessment.estimated_compute_units, 4);
    }

    #[test]
    fn added_compute_cost_and_negative_evidence_cannot_increase_value() {
        let mut decision = workflow();
        decision.expected_uplift_bps = 7_000;
        decision.confidence_bps = 8_000;
        let objective = context("Compare two independent implementation strategies");
        let baseline = assess_auto_computation_value(&decision, &objective, None);

        decision.estimated_steps = 6;
        let higher_cost = assess_auto_computation_value(&decision, &objective, None);
        assert!(higher_cost.required_value_bps >= baseline.required_value_bps);
        assert!(higher_cost.net_value_bps <= baseline.net_value_bps);

        let evidence = MatchedCollaborationEvidence {
            task_class: decision.task_class.clone(),
            effort: "auto".to_string(),
            pre_decision_context_fingerprint: String::new(),
            route_action_id: String::new(),
            routing_signature: decision.learning_signature(),
            examples: 8,
            team_wins: 0,
            team_win_rate: 0.0,
            team_win_confidence: 0.0,
            below_admission_floor: 8,
            below_admission_floor_confidence: 0.67,
            anchor_selections: 8,
            average_uplift_bps: -500,
            average_team_latency_ms: 4_000,
            average_anchor_latency_ms: Some(1_000),
        };
        let evidence_adjusted =
            assess_auto_computation_value(&decision, &objective, Some(&evidence));
        assert!(
            evidence_adjusted.evidence_adjusted_benefit_bps
                <= higher_cost.evidence_adjusted_benefit_bps
        );
        assert!(evidence_adjusted.required_value_bps >= higher_cost.required_value_bps);
    }
}
