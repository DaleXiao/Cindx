use crate::{
    causal_route_action_id_v2, sha256_hex, AgentExecutionMode, AgentRouteTier, AgentRunDecision,
    AgentVerificationPolicy, CausalRouteReason, CausalRouteSelectionV2,
};
use serde::{Deserialize, Serialize};

pub const EXECUTION_PLAN_SCHEMA_V1: &str = "cindx.execution-plan.v1";
pub const EXECUTION_PLAN_GUARD_SCHEMA_V1: &str = "cindx.execution-plan-guard.v1";
pub const EXECUTION_PLAN_PROJECTION_SCHEMA_V1: &str = "cindx.execution-plan-projection.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPlanAuthority {
    FixedPolicy,
    Conductor,
    HardConstraintGuard,
    CompatibilityValuePolicy,
    RuntimeConstraint,
    DegradedFallback,
}

impl ExecutionPlanAuthority {
    pub const fn label(self) -> &'static str {
        match self {
            Self::FixedPolicy => "fixed_policy",
            Self::Conductor => "conductor",
            Self::HardConstraintGuard => "hard_constraint_guard",
            Self::CompatibilityValuePolicy => "compatibility_value_policy",
            Self::RuntimeConstraint => "runtime_constraint",
            Self::DegradedFallback => "degraded_fallback",
        }
    }

    pub const fn from_route_reason(reason: CausalRouteReason) -> Self {
        match reason {
            CausalRouteReason::FastPolicy => Self::FixedPolicy,
            CausalRouteReason::CandidateDirect | CausalRouteReason::AdmitPositiveValue => {
                Self::Conductor
            }
            CausalRouteReason::NoCompatibleRoute | CausalRouteReason::NoIndependentDemand => {
                Self::HardConstraintGuard
            }
            CausalRouteReason::BelowPredictionFloor
            | CausalRouteReason::MatchedEvidenceAgainst
            | CausalRouteReason::NegativeExpectedValue => Self::CompatibilityValuePolicy,
            CausalRouteReason::ExecutionConstraint | CausalRouteReason::SerialInteraction => {
                Self::RuntimeConstraint
            }
            CausalRouteReason::DegradedFallback => Self::DegradedFallback,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPlanGuardReason {
    CandidateDirect,
    Allowed,
    NoCompatibleRoute,
    NoIndependentDemand,
    UnboundedExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlanGuardReceipt {
    pub schema: String,
    pub candidate_action_id: String,
    pub requirements_fingerprint: String,
    pub model_pool_sha256: String,
    pub capability_compatible: bool,
    pub independent_work_valid: bool,
    pub bounded_execution: bool,
    pub allowed: bool,
    pub reason: ExecutionPlanGuardReason,
}

impl ExecutionPlanGuardReceipt {
    pub fn from_candidate(
        candidate: &AgentRunDecision,
        route: &CausalRouteSelectionV2,
    ) -> Result<Self, String> {
        route.validate()?;
        let candidate_action_id = causal_route_action_id_v2(candidate)?;
        let action = route
            .actions
            .iter()
            .find(|action| action.action_id == candidate_action_id)
            .ok_or_else(|| "execution-plan guard cannot find the conductor action".to_string())?;
        let workflow = candidate.route_tier() == AgentRouteTier::Workflow;
        let independent_work_valid = !workflow
            || (candidate.max_parallelism >= 2 && candidate.distinct_contributions >= 2)
            || (candidate.verification == AgentVerificationPolicy::Independent
                && candidate.estimated_steps >= 2);
        let bounded_execution = (1..=5).contains(&candidate.estimated_steps)
            && (1..=3).contains(&candidate.max_parallelism)
            && action.projected_model_calls <= 8;
        let capability_compatible = action.feasible;
        let allowed = capability_compatible && independent_work_valid && bounded_execution;
        let reason = if !workflow {
            ExecutionPlanGuardReason::CandidateDirect
        } else if !capability_compatible {
            ExecutionPlanGuardReason::NoCompatibleRoute
        } else if !independent_work_valid {
            ExecutionPlanGuardReason::NoIndependentDemand
        } else if !bounded_execution {
            ExecutionPlanGuardReason::UnboundedExecution
        } else {
            ExecutionPlanGuardReason::Allowed
        };
        Ok(Self {
            schema: EXECUTION_PLAN_GUARD_SCHEMA_V1.to_string(),
            candidate_action_id,
            requirements_fingerprint: route.requirements_fingerprint.clone(),
            model_pool_sha256: route.model_pool_sha256.clone(),
            capability_compatible,
            independent_work_valid,
            bounded_execution,
            allowed,
            reason,
        })
    }

    pub fn validate(
        &self,
        candidate: &AgentRunDecision,
        route: &CausalRouteSelectionV2,
    ) -> Result<(), String> {
        if self.schema != EXECUTION_PLAN_GUARD_SCHEMA_V1
            || self.requirements_fingerprint != route.requirements_fingerprint
            || self.model_pool_sha256 != route.model_pool_sha256
            || self != &Self::from_candidate(candidate, route)?
        {
            return Err("execution-plan hard-constraint receipt is invalid".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlanProjection {
    pub schema: String,
    pub task_class: crate::TaskClass,
    pub execution: AgentExecutionMode,
    pub selected_action_id: String,
    pub workflow_execution_profile_sha256: Option<String>,
}

impl ExecutionPlanProjection {
    pub fn digest(&self) -> Result<String, String> {
        serde_json::to_vec(self)
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| format!("execution-plan projection serialization failed: {error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlan {
    pub schema: String,
    pub conductor_candidate: AgentRunDecision,
    pub action: AgentRunDecision,
    pub authority: ExecutionPlanAuthority,
    pub hard_guard: ExecutionPlanGuardReceipt,
    pub compatibility_route: CausalRouteSelectionV2,
    pub workflow_execution_profile_sha256: Option<String>,
}

impl ExecutionPlan {
    pub fn new(
        mut conductor_candidate: AgentRunDecision,
        mut action: AgentRunDecision,
        compatibility_route: CausalRouteSelectionV2,
        workflow_execution_profile_sha256: Option<String>,
    ) -> Result<Self, String> {
        conductor_candidate.causal_route = None;
        action.causal_route = None;
        let hard_guard =
            ExecutionPlanGuardReceipt::from_candidate(&conductor_candidate, &compatibility_route)?;
        let authority = ExecutionPlanAuthority::from_route_reason(compatibility_route.reason);
        let plan = Self {
            schema: EXECUTION_PLAN_SCHEMA_V1.to_string(),
            conductor_candidate,
            action,
            authority,
            hard_guard,
            compatibility_route,
            workflow_execution_profile_sha256,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn action(&self) -> &AgentRunDecision {
        &self.action
    }

    pub fn projection(&self) -> ExecutionPlanProjection {
        ExecutionPlanProjection {
            schema: EXECUTION_PLAN_PROJECTION_SCHEMA_V1.to_string(),
            task_class: self.action.task_class.clone(),
            execution: self.action.execution,
            selected_action_id: self.compatibility_route.selected_action_id.clone(),
            workflow_execution_profile_sha256: self.workflow_execution_profile_sha256.clone(),
        }
    }

    pub fn digest(&self) -> Result<String, String> {
        serde_json::to_vec(self)
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| format!("execution-plan serialization failed: {error}"))
    }

    pub fn semantic_digest(&self) -> Result<String, String> {
        self.projection().digest()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != EXECUTION_PLAN_SCHEMA_V1 {
            return Err(format!(
                "unsupported execution-plan schema: {}",
                self.schema
            ));
        }
        self.compatibility_route.validate()?;
        self.hard_guard
            .validate(&self.conductor_candidate, &self.compatibility_route)?;
        let candidate_action_id = causal_route_action_id_v2(&self.conductor_candidate)?;
        let selected_action_id = causal_route_action_id_v2(&self.action)?;
        if candidate_action_id != self.hard_guard.candidate_action_id
            || selected_action_id != self.compatibility_route.selected_action_id
            || self.action.route_tier() != self.compatibility_route.selected_route
            || self.authority
                != ExecutionPlanAuthority::from_route_reason(self.compatibility_route.reason)
        {
            return Err("execution-plan action or authority is inconsistent".to_string());
        }
        match self.action.execution {
            AgentExecutionMode::Direct if self.workflow_execution_profile_sha256.is_some() => {
                return Err(
                    "direct execution must not claim a workflow execution profile".to_string(),
                )
            }
            AgentExecutionMode::Workflow => {
                let digest = self
                    .workflow_execution_profile_sha256
                    .as_deref()
                    .ok_or_else(|| {
                        "workflow execution must identify its executable prompt profile".to_string()
                    })?;
                if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err("workflow execution profile digest is invalid".to_string());
                }
            }
            AgentExecutionMode::Direct => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        select_causal_route_v2, AgentRouteRequirements, ConductorStopPolicy, ModelCandidate,
        ModelCapabilitySource, RouteFeatureRequest, RouteFeatureSnapshotV2,
        MAX_RUN_DECISION_RATIONALE_CHARS,
    };
    use agent_core::ModelRole;

    fn models() -> Vec<ModelCandidate> {
        vec![ModelCandidate {
            name: "executor".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: true,
            tools_capability_source: ModelCapabilitySource::Configured,
            vision_capability_source: ModelCapabilitySource::Configured,
            cost_tier: 1,
            latency_tier: 1,
        }]
    }

    fn workflow_candidate(expected_uplift_bps: u16) -> AgentRunDecision {
        let mut decision = AgentRunDecision::direct("executor");
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::Independent;
        decision.max_parallelism = 2;
        decision.min_successful_branches = 2;
        decision.distinct_contributions = 2;
        decision.estimated_steps = 3;
        decision.expected_uplift_bps = expected_uplift_bps;
        decision.confidence_bps = 8_000;
        decision.stop_policy = ConductorStopPolicy::Quorum;
        decision.rationale = "two independently checkable contributions".to_string();
        decision
    }

    fn route(candidate: &AgentRunDecision) -> CausalRouteSelectionV2 {
        let models = models();
        let snapshot = RouteFeatureSnapshotV2::from_decision_request(
            candidate,
            RouteFeatureRequest {
                objective: "compare two implementation strategies",
                recent_context: "",
                effort: "auto",
                requirements: AgentRouteRequirements::default(),
                budget_fingerprint: Some(&"0".repeat(64)),
                prompt_profile_sha256: &"1".repeat(64),
            },
            &models,
        );
        select_causal_route_v2(candidate, &snapshot, &models, None, 0).expect("route receipt")
    }

    #[test]
    fn admitted_workflow_keeps_conductor_as_the_decision_authority() {
        let candidate = workflow_candidate(6_000);
        let receipt = route(&candidate);
        assert_eq!(receipt.reason, CausalRouteReason::AdmitPositiveValue);

        let plan = ExecutionPlan::new(candidate.clone(), candidate, receipt, Some("a".repeat(64)))
            .expect("execution plan");

        assert_eq!(plan.authority, ExecutionPlanAuthority::Conductor);
        assert!(plan.hard_guard.allowed);
        assert_eq!(plan.action.execution, AgentExecutionMode::Workflow);
    }

    #[test]
    fn compatibility_value_downshift_is_explicit_and_preserves_the_candidate() {
        let candidate = workflow_candidate(2_999);
        let receipt = route(&candidate);
        assert_eq!(receipt.reason, CausalRouteReason::BelowPredictionFloor);
        let selected = candidate.clone().constrained_to_grounded_direct();

        let plan = ExecutionPlan::new(candidate, selected, receipt, None)
            .expect("compatibility execution plan");

        assert_eq!(
            plan.authority,
            ExecutionPlanAuthority::CompatibilityValuePolicy
        );
        assert!(plan.hard_guard.allowed);
        assert_eq!(
            plan.conductor_candidate.execution,
            AgentExecutionMode::Workflow
        );
        assert_eq!(plan.action.execution, AgentExecutionMode::Direct);
    }

    #[test]
    fn hard_constraint_rejection_is_distinct_from_a_quality_downshift() {
        let mut candidate = workflow_candidate(6_000);
        candidate.verification = AgentVerificationPolicy::SelfCheck;
        candidate.max_parallelism = 1;
        candidate.min_successful_branches = 1;
        candidate.distinct_contributions = 0;
        let receipt = route(&candidate);
        assert_eq!(receipt.reason, CausalRouteReason::NoIndependentDemand);

        let plan = ExecutionPlan::new(
            candidate.clone(),
            candidate.constrained_to_grounded_direct(),
            receipt,
            None,
        )
        .expect("guarded execution plan");

        assert_eq!(plan.authority, ExecutionPlanAuthority::HardConstraintGuard);
        assert!(!plan.hard_guard.allowed);
        assert_eq!(
            plan.hard_guard.reason,
            ExecutionPlanGuardReason::NoIndependentDemand
        );
    }

    #[test]
    fn semantic_identity_ignores_explanatory_rationale_but_full_digest_does_not() {
        let candidate = workflow_candidate(6_000);
        let plan = ExecutionPlan::new(
            candidate.clone(),
            candidate,
            route(&workflow_candidate(6_000)),
            Some("a".repeat(64)),
        )
        .expect("execution plan");
        let mut reworded = plan.clone();
        reworded.conductor_candidate.rationale = "same plan, reworded".to_string();
        reworded.action.rationale = "same plan, reworded".to_string();
        reworded.validate().expect("reworded plan remains valid");

        assert_ne!(plan.digest().unwrap(), reworded.digest().unwrap());
        assert_eq!(
            plan.semantic_digest().unwrap(),
            reworded.semantic_digest().unwrap()
        );
    }

    #[test]
    fn execution_plan_receipt_and_semantic_projection_remain_bounded() {
        let mut candidate = workflow_candidate(6_000);
        candidate.rationale = "r".repeat(MAX_RUN_DECISION_RATIONALE_CHARS);
        let plan = ExecutionPlan::new(
            candidate.clone(),
            candidate.clone(),
            route(&candidate),
            Some("a".repeat(64)),
        )
        .expect("maximal valid execution plan");

        let receipt_bytes = serde_json::to_vec(&plan).expect("execution plan receipt");
        let projection_bytes =
            serde_json::to_vec(&plan.projection()).expect("execution plan projection");

        assert!(receipt_bytes.len() <= 12 * 1024, "{}", receipt_bytes.len());
        assert!(projection_bytes.len() <= 512, "{}", projection_bytes.len());
        assert!(plan.compatibility_route.actions.len() <= 2);
    }
}
