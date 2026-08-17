use crate::{
    causal_route_action_id_v2, sha256_hex, AgentExecutionMode, AgentRouteTier, AgentRunDecision,
    AgentVerificationPolicy, CausalRouteReason, CausalRouteSelectionV2,
};
use serde::{Deserialize, Serialize};

pub const EXECUTION_PLAN_SCHEMA_V1: &str = "cindx.execution-plan.v1";
pub const EXECUTION_PLAN_SCHEMA_V2: &str = "cindx.execution-plan.v2";
pub const EXECUTION_PLAN_GUARD_SCHEMA_V1: &str = "cindx.execution-plan-guard.v1";
pub const EXECUTION_PLAN_GUARD_SCHEMA_V2: &str = "cindx.execution-plan-guard.v2";
pub const EXECUTION_PLAN_PROJECTION_SCHEMA_V1: &str = "cindx.execution-plan-projection.v1";
pub const EXECUTION_PLAN_PROJECTION_SCHEMA_V2: &str = "cindx.execution-plan-projection.v2";
pub const EXECUTION_PLAN_DECISION_RECEIPT_SCHEMA_V1: &str =
    "cindx.execution-plan-decision-receipt.v1";

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
pub enum ExecutionPlanDecisionReason {
    FastPolicy,
    ConductorSelection,
    HardSafetyConstraint,
    HardCapabilityConstraint,
    HardBudgetConstraint,
    RuntimeGroundedDirect,
    MatchedMemoryEvaluation,
    MatchedRouteEvaluation,
    DegradedFallback,
}

impl ExecutionPlanDecisionReason {
    pub const fn authority(self) -> ExecutionPlanAuthority {
        match self {
            Self::FastPolicy => ExecutionPlanAuthority::FixedPolicy,
            Self::ConductorSelection => ExecutionPlanAuthority::Conductor,
            Self::HardSafetyConstraint
            | Self::HardCapabilityConstraint
            | Self::HardBudgetConstraint => ExecutionPlanAuthority::HardConstraintGuard,
            Self::RuntimeGroundedDirect
            | Self::MatchedMemoryEvaluation
            | Self::MatchedRouteEvaluation => ExecutionPlanAuthority::RuntimeConstraint,
            Self::DegradedFallback => ExecutionPlanAuthority::DegradedFallback,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::FastPolicy => "fast_policy",
            Self::ConductorSelection => "conductor_selection",
            Self::HardSafetyConstraint => "hard_safety_constraint",
            Self::HardCapabilityConstraint => "hard_capability_constraint",
            Self::HardBudgetConstraint => "hard_budget_constraint",
            Self::RuntimeGroundedDirect => "runtime_grounded_direct",
            Self::MatchedMemoryEvaluation => "matched_memory_evaluation",
            Self::MatchedRouteEvaluation => "matched_route_evaluation",
            Self::DegradedFallback => "degraded_fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlanDecisionReceipt {
    pub schema: String,
    pub authority: ExecutionPlanAuthority,
    pub reason: ExecutionPlanDecisionReason,
    pub conductor_action_id: String,
    pub executable_action_id: String,
    pub action_changed: bool,
}

impl ExecutionPlanDecisionReceipt {
    pub fn new(
        candidate: &AgentRunDecision,
        action: &AgentRunDecision,
        reason: ExecutionPlanDecisionReason,
    ) -> Result<Self, String> {
        let conductor_action_id = causal_route_action_id_v2(candidate)?;
        let executable_action_id = causal_route_action_id_v2(action)?;
        Ok(Self {
            schema: EXECUTION_PLAN_DECISION_RECEIPT_SCHEMA_V1.to_string(),
            authority: reason.authority(),
            reason,
            action_changed: conductor_action_id != executable_action_id,
            conductor_action_id,
            executable_action_id,
        })
    }

    pub fn validate(
        &self,
        candidate: &AgentRunDecision,
        action: &AgentRunDecision,
    ) -> Result<(), String> {
        if self.schema != EXECUTION_PLAN_DECISION_RECEIPT_SCHEMA_V1
            || self != &Self::new(candidate, action, self.reason)?
        {
            return Err("execution-plan decision receipt is invalid".to_string());
        }
        Ok(())
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
        Self::from_candidate_for_schema(candidate, route, EXECUTION_PLAN_GUARD_SCHEMA_V2)
    }

    fn from_candidate_for_schema(
        candidate: &AgentRunDecision,
        route: &CausalRouteSelectionV2,
        schema: &str,
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
        let legacy = schema == EXECUTION_PLAN_GUARD_SCHEMA_V1;
        let allowed =
            capability_compatible && bounded_execution && (!legacy || independent_work_valid);
        let reason = if !workflow {
            ExecutionPlanGuardReason::CandidateDirect
        } else if !capability_compatible {
            ExecutionPlanGuardReason::NoCompatibleRoute
        } else if legacy && !independent_work_valid {
            ExecutionPlanGuardReason::NoIndependentDemand
        } else if !bounded_execution {
            ExecutionPlanGuardReason::UnboundedExecution
        } else {
            ExecutionPlanGuardReason::Allowed
        };
        Ok(Self {
            schema: schema.to_string(),
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
        if !matches!(
            self.schema.as_str(),
            EXECUTION_PLAN_GUARD_SCHEMA_V1 | EXECUTION_PLAN_GUARD_SCHEMA_V2
        ) || self.requirements_fingerprint != route.requirements_fingerprint
            || self.model_pool_sha256 != route.model_pool_sha256
            || self != &Self::from_candidate_for_schema(candidate, route, &self.schema)?
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_receipt: Option<ExecutionPlanDecisionReceipt>,
    pub compatibility_route: CausalRouteSelectionV2,
    pub workflow_execution_profile_sha256: Option<String>,
}

impl ExecutionPlan {
    pub fn new(
        mut conductor_candidate: AgentRunDecision,
        mut action: AgentRunDecision,
        decision_reason: ExecutionPlanDecisionReason,
        compatibility_route: CausalRouteSelectionV2,
        workflow_execution_profile_sha256: Option<String>,
    ) -> Result<Self, String> {
        conductor_candidate.causal_route = None;
        action.causal_route = None;
        let hard_guard =
            ExecutionPlanGuardReceipt::from_candidate(&conductor_candidate, &compatibility_route)?;
        let decision_receipt =
            ExecutionPlanDecisionReceipt::new(&conductor_candidate, &action, decision_reason)?;
        let authority = decision_receipt.authority;
        let plan = Self {
            schema: EXECUTION_PLAN_SCHEMA_V2.to_string(),
            conductor_candidate,
            action,
            authority,
            hard_guard,
            decision_receipt: Some(decision_receipt),
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
        let legacy = self.schema == EXECUTION_PLAN_SCHEMA_V1;
        ExecutionPlanProjection {
            schema: if legacy {
                EXECUTION_PLAN_PROJECTION_SCHEMA_V1
            } else {
                EXECUTION_PLAN_PROJECTION_SCHEMA_V2
            }
            .to_string(),
            task_class: self.action.task_class.clone(),
            execution: self.action.execution,
            selected_action_id: if legacy {
                self.compatibility_route.selected_action_id.clone()
            } else {
                causal_route_action_id_v2(&self.action)
                    .expect("validated execution action identity must serialize")
            },
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
        if !matches!(
            self.schema.as_str(),
            EXECUTION_PLAN_SCHEMA_V1 | EXECUTION_PLAN_SCHEMA_V2
        ) {
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
        if candidate_action_id != self.hard_guard.candidate_action_id {
            return Err("execution-plan candidate is inconsistent".to_string());
        }
        if self.schema == EXECUTION_PLAN_SCHEMA_V1 {
            if self.decision_receipt.is_some()
                || selected_action_id != self.compatibility_route.selected_action_id
                || self.action.route_tier() != self.compatibility_route.selected_route
                || self.authority
                    != ExecutionPlanAuthority::from_route_reason(self.compatibility_route.reason)
            {
                return Err("legacy execution-plan action or authority is inconsistent".to_string());
            }
        } else {
            let decision_receipt = self
                .decision_receipt
                .as_ref()
                .ok_or_else(|| "execution-plan decision receipt is missing".to_string())?;
            decision_receipt.validate(&self.conductor_candidate, &self.action)?;
            if self.authority != decision_receipt.authority
                || !self
                    .compatibility_route
                    .actions
                    .iter()
                    .any(|action| action.action_id == selected_action_id && action.feasible)
                || self.compatibility_route.candidate_route != self.conductor_candidate.route_tier()
            {
                return Err("execution-plan action or authority is inconsistent".to_string());
            }
            match self.authority {
                ExecutionPlanAuthority::Conductor
                | ExecutionPlanAuthority::FixedPolicy
                | ExecutionPlanAuthority::DegradedFallback
                    if selected_action_id != candidate_action_id =>
                {
                    return Err("execution-plan authority cannot rewrite its candidate".to_string())
                }
                ExecutionPlanAuthority::HardConstraintGuard if self.hard_guard.allowed => {
                    return Err(
                        "hard-constraint authority requires a rejected candidate".to_string()
                    )
                }
                ExecutionPlanAuthority::CompatibilityValuePolicy => {
                    return Err(
                        "compatibility value policy is shadow-only in execution-plan v2"
                            .to_string(),
                    )
                }
                _ if self.authority != ExecutionPlanAuthority::HardConstraintGuard
                    && !self.hard_guard.allowed =>
                {
                    return Err("execution-plan bypasses a hard constraint".to_string())
                }
                _ => {}
            }
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

        let plan = ExecutionPlan::new(
            candidate.clone(),
            candidate,
            ExecutionPlanDecisionReason::ConductorSelection,
            receipt,
            Some("a".repeat(64)),
        )
        .expect("execution plan");

        assert_eq!(plan.authority, ExecutionPlanAuthority::Conductor);
        assert!(plan.hard_guard.allowed);
        assert_eq!(plan.action.execution, AgentExecutionMode::Workflow);
    }

    #[test]
    fn quarantine_clamp_recomputes_route_so_plan_stays_consistent() {
        let candidate = workflow_candidate(6_000);
        let stale_route = route(&candidate);

        // A workflow route receipt validated against a clamped-direct action is
        // inconsistent and must be rejected.
        let clamped_action = candidate.clone().constrained_to_grounded_direct();
        assert!(ExecutionPlan::new(
            candidate.clone(),
            clamped_action.clone(),
            ExecutionPlanDecisionReason::ConductorSelection,
            stale_route,
            None,
        )
        .is_err());

        // Clamping the recorded candidate identically and recomputing the route
        // from it yields a consistent, validated direct plan.
        let clamped_candidate = candidate.constrained_to_grounded_direct();
        let fresh_route = route(&clamped_candidate);
        let plan = ExecutionPlan::new(
            clamped_candidate,
            clamped_action,
            ExecutionPlanDecisionReason::ConductorSelection,
            fresh_route,
            None,
        )
        .expect("clamped candidate plus recomputed route validates");
        assert_eq!(plan.action.execution, AgentExecutionMode::Direct);
        assert_eq!(plan.authority, ExecutionPlanAuthority::Conductor);
    }

    #[test]
    fn compatibility_value_policy_is_shadow_only_for_v2() {
        let candidate = workflow_candidate(2_999);
        let receipt = route(&candidate);
        assert_eq!(receipt.reason, CausalRouteReason::BelowPredictionFloor);

        let plan = ExecutionPlan::new(
            candidate.clone(),
            candidate,
            ExecutionPlanDecisionReason::ConductorSelection,
            receipt,
            Some("a".repeat(64)),
        )
        .expect("conductor-owned execution plan");

        assert_eq!(plan.authority, ExecutionPlanAuthority::Conductor);
        assert!(plan.hard_guard.allowed);
        assert_eq!(plan.action.execution, AgentExecutionMode::Workflow);
        assert_eq!(
            plan.compatibility_route.selected_route,
            AgentRouteTier::Direct,
            "the shadow recommendation remains auditable"
        );
        assert_eq!(
            plan.decision_receipt.as_ref().map(|receipt| receipt.reason),
            Some(ExecutionPlanDecisionReason::ConductorSelection)
        );
    }

    #[test]
    fn independent_work_observation_is_not_a_hard_override() {
        let mut candidate = workflow_candidate(6_000);
        candidate.verification = AgentVerificationPolicy::SelfCheck;
        candidate.max_parallelism = 1;
        candidate.min_successful_branches = 1;
        candidate.distinct_contributions = 1;
        let receipt = route(&candidate);
        assert_eq!(receipt.reason, CausalRouteReason::NoIndependentDemand);

        let plan = ExecutionPlan::new(
            candidate.clone(),
            candidate,
            ExecutionPlanDecisionReason::ConductorSelection,
            receipt,
            Some("a".repeat(64)),
        )
        .expect("independent-demand observation must remain shadow-only");

        assert_eq!(plan.authority, ExecutionPlanAuthority::Conductor);
        assert!(plan.hard_guard.allowed);
        assert!(!plan.hard_guard.independent_work_valid);
        assert_eq!(plan.hard_guard.reason, ExecutionPlanGuardReason::Allowed);
    }

    #[test]
    fn explicit_runtime_override_has_a_bound_decision_receipt() {
        let candidate = workflow_candidate(6_000);
        let action = candidate.clone().constrained_to_grounded_direct();
        let plan = ExecutionPlan::new(
            candidate,
            action,
            ExecutionPlanDecisionReason::RuntimeGroundedDirect,
            route(&workflow_candidate(6_000)),
            None,
        )
        .expect("runtime constraint plan");

        let receipt = plan.decision_receipt.as_ref().expect("decision receipt");
        assert_eq!(plan.authority, ExecutionPlanAuthority::RuntimeConstraint);
        assert_eq!(
            receipt.reason,
            ExecutionPlanDecisionReason::RuntimeGroundedDirect
        );
        assert!(receipt.action_changed);
    }

    #[test]
    fn conductor_authority_cannot_rewrite_its_candidate() {
        let candidate = workflow_candidate(6_000);
        let action = candidate.clone().constrained_to_grounded_direct();

        let error = ExecutionPlan::new(
            candidate.clone(),
            action,
            ExecutionPlanDecisionReason::ConductorSelection,
            route(&candidate),
            None,
        )
        .expect_err("conductor authority must preserve its selected action");

        assert!(error.contains("authority cannot rewrite its candidate"));
    }

    #[test]
    fn legacy_v1_compatibility_downshift_remains_replayable() {
        let candidate = workflow_candidate(2_999);
        let receipt = route(&candidate);
        let action = candidate.clone().constrained_to_grounded_direct();
        let mut hard_guard = ExecutionPlanGuardReceipt::from_candidate_for_schema(
            &candidate,
            &receipt,
            EXECUTION_PLAN_GUARD_SCHEMA_V1,
        )
        .expect("legacy guard");
        hard_guard.schema = EXECUTION_PLAN_GUARD_SCHEMA_V1.to_string();
        let plan = ExecutionPlan {
            schema: EXECUTION_PLAN_SCHEMA_V1.to_string(),
            conductor_candidate: candidate,
            action,
            authority: ExecutionPlanAuthority::CompatibilityValuePolicy,
            hard_guard,
            decision_receipt: None,
            compatibility_route: receipt,
            workflow_execution_profile_sha256: None,
        };

        let encoded = serde_json::to_string(&plan).expect("legacy plan json");
        let decoded = serde_json::from_str::<ExecutionPlan>(&encoded).expect("legacy plan decode");
        decoded.validate().expect("legacy plan validation");
    }

    #[test]
    fn semantic_identity_ignores_explanatory_rationale_but_full_digest_does_not() {
        let candidate = workflow_candidate(6_000);
        let plan = ExecutionPlan::new(
            candidate.clone(),
            candidate,
            ExecutionPlanDecisionReason::ConductorSelection,
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
            ExecutionPlanDecisionReason::ConductorSelection,
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
