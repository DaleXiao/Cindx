use crate::{
    minimum_team_uplift_bps, AgentEffectAuthority, AgentRiskLevel, AgentRouteRequirements,
    AgentRouteTier, AgentRunDecision, AgentToolRequirement, AgentVerificationPolicy,
    ConductorStopPolicy, MatchedCollaborationEvidence, MemoryRecallPolicy, ModelCandidate,
    ModelCapabilitySource, RoutingContext, TaskClass, WorkspaceRetrievalChannel,
    AUTO_COLLABORATION_MIN_CONFIDENCE_BPS, AUTO_COLLABORATION_MIN_UPLIFT_BPS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[cfg(test)]
use crate::MatchedCollaborationEvidenceTeacher;

pub const CAUSAL_ROUTE_SELECTION_SCHEMA_V2: &str = "cindx.causal-route.v2";
pub const CAUSAL_ROUTE_SELECTION_POLICY_V2: &str = "runtime-counterfactual-value-v2";
pub const CAUSAL_ROUTE_MAX_ACTIONS: usize = 2;
pub const CAUSAL_ROUTE_MAX_PROMPT_EVIDENCE_ROWS: usize = 8;
pub const CAUSAL_ROUTE_MAX_RECEIPT_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteCapabilityEvidence {
    Supported,
    Unsupported,
    CompatibilityAssumed,
}

impl RouteCapabilityEvidence {
    fn from_candidate(supported: bool, source: ModelCapabilitySource) -> Self {
        match (supported, source) {
            (false, _) => Self::Unsupported,
            (true, ModelCapabilitySource::CompatibilityAssumption) => Self::CompatibilityAssumed,
            (true, _) => Self::Supported,
        }
    }

    fn usable(self) -> bool {
        self != Self::Unsupported
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalRouteEvidenceBasis {
    RuntimeDemandOnly,
    LegacyMatchedAction,
    MatchedContextAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalRouteReason {
    FastPolicy,
    CandidateDirect,
    AdmitPositiveValue,
    BelowPredictionFloor,
    NoIndependentDemand,
    SerialInteraction,
    MatchedEvidenceAgainst,
    NegativeExpectedValue,
    NoCompatibleRoute,
    ExecutionConstraint,
    DegradedFallback,
}

impl CausalRouteReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::FastPolicy => "fast_policy",
            Self::CandidateDirect => "candidate_direct",
            Self::AdmitPositiveValue => "admit_positive_value",
            Self::BelowPredictionFloor => "below_prediction_floor",
            Self::NoIndependentDemand => "no_independent_demand",
            Self::SerialInteraction => "serial_interaction",
            Self::MatchedEvidenceAgainst => "matched_evidence_against",
            Self::NegativeExpectedValue => "negative_expected_value",
            Self::NoCompatibleRoute => "no_compatible_route",
            Self::ExecutionConstraint => "execution_constraint",
            Self::DegradedFallback => "degraded_fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteFeatureSnapshotV2 {
    pub schema: String,
    pub effort: String,
    pub objective_sha256: String,
    pub recent_context_sha256: String,
    pub prompt_profile_sha256: String,
    pub task_class: TaskClass,
    pub prompt_length_bucket: u16,
    pub minimum_tool_requirement: AgentToolRequirement,
    pub effect_authority: AgentEffectAuthority,
    pub image_input_required: bool,
    pub needs_retrieval: bool,
    pub high_stakes: bool,
    pub parallelizable: bool,
    pub verification_required: bool,
    pub latency_sensitive: bool,
    pub complexity_band: u8,
    pub estimated_steps: u8,
    pub model_pool_sha256: String,
    pub model_pool_size: usize,
    pub budget_fingerprint: Option<String>,
    pub requirements_fingerprint: String,
    pub context_fingerprint: String,
}

impl RouteFeatureSnapshotV2 {
    pub fn from_request(
        objective: &str,
        recent_context: &str,
        effort: &str,
        requirements: AgentRouteRequirements,
        model_candidates: &[ModelCandidate],
        budget_fingerprint: Option<String>,
        prompt_profile_sha256: String,
    ) -> Self {
        let context = RoutingContext::from_prompt(objective, model_candidates.to_vec());
        let effort = normalize_effort(effort);
        let objective_sha256 = sha256_hex(objective.as_bytes());
        let recent_context_sha256 = sha256_hex(recent_context.as_bytes());
        let prompt_length_bucket = context.prompt_length.min(4_096).div_ceil(128) as u16;
        let model_pool_sha256 = model_pool_digest(model_candidates);
        let requirements_fingerprint = sha256_hex(
            format!(
                "tool={};effects={};image={}",
                requirements.minimum_tool_requirement.label(),
                requirements.effect_authority.label(),
                u8::from(requirements.image_input_required),
            )
            .as_bytes(),
        );
        let canonical = format!(
            "schema={CAUSAL_ROUTE_SELECTION_SCHEMA_V2};effort={effort};objective={objective_sha256};recent={recent_context_sha256};profile={prompt_profile_sha256};class={};length={prompt_length_bucket};tool={};effects={};image={};retrieval={};stakes={};parallel={};verify={};latency={};complexity={};steps={};models={model_pool_sha256};budget={}",
            context.task_class.label(),
            requirements.minimum_tool_requirement.label(),
            requirements.effect_authority.label(),
            u8::from(requirements.image_input_required),
            u8::from(context.needs_retrieval),
            u8::from(context.high_stakes),
            u8::from(context.parallelizable),
            u8::from(context.verification_required),
            u8::from(context.latency_sensitive),
            (context.complexity_score / 2).min(4),
            context.estimated_steps.min(5),
            budget_fingerprint.as_deref().unwrap_or("missing"),
        );
        Self {
            schema: CAUSAL_ROUTE_SELECTION_SCHEMA_V2.to_string(),
            effort,
            objective_sha256,
            recent_context_sha256,
            prompt_profile_sha256,
            task_class: context.task_class,
            prompt_length_bucket,
            minimum_tool_requirement: requirements.minimum_tool_requirement,
            effect_authority: requirements.effect_authority,
            image_input_required: requirements.image_input_required,
            needs_retrieval: context.needs_retrieval,
            high_stakes: context.high_stakes,
            parallelizable: context.parallelizable,
            verification_required: context.verification_required,
            latency_sensitive: context.latency_sensitive,
            complexity_band: (context.complexity_score / 2).min(4),
            estimated_steps: context.estimated_steps.min(5),
            model_pool_sha256,
            model_pool_size: unique_model_count(model_candidates),
            budget_fingerprint,
            requirements_fingerprint,
            context_fingerprint: sha256_hex(canonical.as_bytes()),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != CAUSAL_ROUTE_SELECTION_SCHEMA_V2
            || !is_sha256(&self.objective_sha256)
            || !is_sha256(&self.recent_context_sha256)
            || !is_sha256(&self.prompt_profile_sha256)
            || !is_sha256(&self.model_pool_sha256)
            || !is_sha256(&self.requirements_fingerprint)
            || !is_sha256(&self.context_fingerprint)
            || self.model_pool_size == 0
        {
            return Err("causal route feature snapshot is invalid".to_string());
        }
        if self
            .budget_fingerprint
            .as_ref()
            .is_some_and(|digest| !is_sha256(digest))
        {
            return Err("causal route budget fingerprint is invalid".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalRouteActionV2 {
    pub action_id: String,
    pub route_tier: AgentRouteTier,
    pub model: String,
    pub tools: RouteCapabilityEvidence,
    pub vision: RouteCapabilityEvidence,
    pub projected_model_calls: u8,
    pub cost_units: u16,
    pub critical_path_latency_units: u16,
    pub feasible: bool,
}

#[derive(Serialize)]
struct RouteActionIdentityV2 {
    schema: &'static str,
    route_tier: AgentRouteTier,
    primary_model: String,
    tool_requirement: AgentToolRequirement,
    vision_required: bool,
    risk_level: AgentRiskLevel,
    retrieval_channels: BTreeSet<WorkspaceRetrievalChannel>,
    retrieval_max_results: usize,
    retrieval_query_sha256: String,
    memory_policy: MemoryRecallPolicy,
    memory_query_sha256: String,
    verification: AgentVerificationPolicy,
    max_parallelism: usize,
    min_successful_branches: usize,
    distinct_contributions: usize,
    estimated_steps: usize,
    stop_policy: ConductorStopPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalRouteSupportV2 {
    pub basis: CausalRouteEvidenceBasis,
    pub evidence_sha256: Option<String>,
    pub matched_examples: usize,
    pub observed_uplift_bps: Option<i16>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalRouteOperationCounts {
    pub candidate_evaluations: usize,
    pub evidence_key_lookups: usize,
    pub historical_rows_scanned: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalRouteSelectionV2 {
    pub schema: String,
    pub policy: String,
    pub feature_snapshot: RouteFeatureSnapshotV2,
    pub context_fingerprint: String,
    pub requirements_fingerprint: String,
    pub model_pool_sha256: String,
    pub candidate_route: AgentRouteTier,
    pub selected_route: AgentRouteTier,
    pub actions: Vec<CausalRouteActionV2>,
    pub selected_action_id: String,
    pub counterfactual_action_id: Option<String>,
    pub support: CausalRouteSupportV2,
    pub predicted_benefit_bps: i32,
    pub evidence_adjusted_benefit_bps: i32,
    pub coordination_cost_bps: i32,
    pub latency_cost_bps: i32,
    pub uncertainty_bps: i32,
    pub net_value_lower_bps: i32,
    pub reason: CausalRouteReason,
    pub deterministic: bool,
    pub propensity_bps: Option<u16>,
    pub operations: CausalRouteOperationCounts,
}

impl CausalRouteSelectionV2 {
    pub fn admitted(&self) -> bool {
        self.candidate_route != AgentRouteTier::Workflow
            || self.selected_route == AgentRouteTier::Workflow
    }

    pub fn digest(&self) -> Result<String, String> {
        serde_json::to_vec(self)
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| format!("causal route receipt serialization failed: {error}"))
    }

    pub fn encoded_len(&self) -> Result<usize, String> {
        serde_json::to_vec(self)
            .map(|encoded| encoded.len())
            .map_err(|error| format!("causal route receipt serialization failed: {error}"))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != CAUSAL_ROUTE_SELECTION_SCHEMA_V2
            || self.policy != CAUSAL_ROUTE_SELECTION_POLICY_V2
            || self.feature_snapshot.validate().is_err()
            || self.feature_snapshot.context_fingerprint != self.context_fingerprint
            || self.feature_snapshot.requirements_fingerprint != self.requirements_fingerprint
            || self.feature_snapshot.model_pool_sha256 != self.model_pool_sha256
            || !is_sha256(&self.context_fingerprint)
            || !is_sha256(&self.requirements_fingerprint)
            || !is_sha256(&self.model_pool_sha256)
            || self.actions.is_empty()
            || self.actions.len() > CAUSAL_ROUTE_MAX_ACTIONS
            || self.encoded_len()? > CAUSAL_ROUTE_MAX_RECEIPT_BYTES
            || self.propensity_bps.is_some()
            || !self.deterministic
            || self.operations.candidate_evaluations > self.actions.len()
            || self.operations.evidence_key_lookups > 2
            || self.operations.historical_rows_scanned != 0
        {
            return Err("causal route receipt contract is invalid".to_string());
        }
        let selected = self
            .actions
            .iter()
            .find(|action| action.action_id == self.selected_action_id)
            .ok_or_else(|| "causal route selected action is missing".to_string())?;
        if selected.route_tier != self.selected_route || !selected.feasible {
            return Err("causal route selected action is inconsistent".to_string());
        }
        if self
            .counterfactual_action_id
            .as_ref()
            .is_some_and(|id| !self.actions.iter().any(|action| &action.action_id == id))
            || self
                .support
                .evidence_sha256
                .as_ref()
                .is_some_and(|digest| !is_sha256(digest))
        {
            return Err("causal route counterfactual or evidence digest is invalid".to_string());
        }
        Ok(())
    }

    pub fn reconcile_selected_route(
        &mut self,
        route: AgentRouteTier,
        reason: CausalRouteReason,
    ) -> Result<(), String> {
        if self.selected_route == route {
            return Ok(());
        }
        let Some(action) = self
            .actions
            .iter()
            .find(|action| action.route_tier == route)
        else {
            return Err("execution constraint selected an unrecorded route".to_string());
        };
        self.counterfactual_action_id = Some(self.selected_action_id.clone());
        self.selected_action_id = action.action_id.clone();
        self.selected_route = route;
        self.reason = reason;
        self.validate()
    }
}

pub fn select_causal_route_v2(
    decision: &AgentRunDecision,
    snapshot: &RouteFeatureSnapshotV2,
    model_candidates: &[ModelCandidate],
    matched: Option<&MatchedCollaborationEvidence>,
    evidence_key_lookups: usize,
) -> Result<CausalRouteSelectionV2, String> {
    snapshot.validate()?;
    let candidate_route = decision.route_tier();
    let candidate_action = route_action(decision, model_candidates)?;
    let fallback_action = if candidate_route == AgentRouteTier::Workflow {
        let fallback = decision.clone().constrained_to_grounded_direct();
        Some(route_action(&fallback, model_candidates)?)
    } else {
        None
    };
    let support = route_support(snapshot, decision, &candidate_action.action_id, matched);
    let predicted_benefit_bps = demand_benefit_bps(snapshot, decision);
    let evidence_adjusted_benefit_bps = match support.basis {
        CausalRouteEvidenceBasis::MatchedContextAction => support
            .observed_uplift_bps
            .map(i32::from)
            .map(|observed| observed.min(predicted_benefit_bps))
            .unwrap_or(predicted_benefit_bps),
        CausalRouteEvidenceBasis::RuntimeDemandOnly
        | CausalRouteEvidenceBasis::LegacyMatchedAction => predicted_benefit_bps,
    };
    let pro = snapshot.effort == "pro";
    let coordination_unit_bps = if pro { 140 } else { 260 };
    let latency_unit_bps = if pro { 70 } else { 160 };
    let coordination_cost_bps = i32::from(candidate_action.cost_units)
        .saturating_mul(coordination_unit_bps)
        .saturating_add(
            i32::from(decision.verification == AgentVerificationPolicy::Independent)
                * if pro { 120 } else { 240 },
        );
    let latency_cost_bps = i32::from(candidate_action.critical_path_latency_units)
        .saturating_mul(latency_unit_bps)
        .saturating_add(i32::from(snapshot.latency_sensitive) * if pro { 300 } else { 900 });
    let assumed_capability = candidate_action.tools
        == RouteCapabilityEvidence::CompatibilityAssumed
        || candidate_action.vision == RouteCapabilityEvidence::CompatibilityAssumed;
    let uncertainty_bps = match support.basis {
        CausalRouteEvidenceBasis::MatchedContextAction => {
            (1_200i32 / support.matched_examples.max(1) as i32).max(100)
        }
        CausalRouteEvidenceBasis::LegacyMatchedAction => {
            if pro {
                500
            } else {
                1_000
            }
        }
        CausalRouteEvidenceBasis::RuntimeDemandOnly => {
            if pro {
                650
            } else {
                1_250
            }
        }
    }
    .saturating_add(i32::from(assumed_capability) * 350);
    let net_value_lower_bps = evidence_adjusted_benefit_bps
        .saturating_sub(coordination_cost_bps)
        .saturating_sub(latency_cost_bps)
        .saturating_sub(uncertainty_bps);
    let prediction_floor_met = if pro {
        decision.expected_uplift_bps >= minimum_team_uplift_bps("pro")
    } else {
        decision.expected_uplift_bps >= AUTO_COLLABORATION_MIN_UPLIFT_BPS
            && decision.confidence_bps >= AUTO_COLLABORATION_MIN_CONFIDENCE_BPS
    };
    let independent_demand = decision.max_parallelism >= 2
        && decision.distinct_contributions >= 2
        && (snapshot.parallelizable
            || decision.verification == AgentVerificationPolicy::Independent
            || (matches!(
                snapshot.task_class,
                TaskClass::Research | TaskClass::Retrieval
            ) && decision.estimated_steps >= 3));
    let serial_interaction = matches!(
        snapshot.task_class,
        TaskClass::Browser | TaskClass::Computer
    ) || decision.tool_requirement == AgentToolRequirement::Effects;
    let serial_exception = snapshot.high_stakes
        && decision.verification == AgentVerificationPolicy::Independent
        && decision.tool_requirement != AgentToolRequirement::Effects;
    let strong_evidence_against = support.basis != CausalRouteEvidenceBasis::RuntimeDemandOnly
        && matched.is_some_and(|evidence| {
            evidence.strong_evidence_against_collaboration(if pro {
                minimum_team_uplift_bps("pro")
            } else {
                AUTO_COLLABORATION_MIN_UPLIFT_BPS
            })
        });
    let reason = if candidate_route != AgentRouteTier::Workflow {
        CausalRouteReason::CandidateDirect
    } else if !candidate_action.feasible {
        CausalRouteReason::NoCompatibleRoute
    } else if !prediction_floor_met {
        CausalRouteReason::BelowPredictionFloor
    } else if !independent_demand {
        CausalRouteReason::NoIndependentDemand
    } else if serial_interaction && !serial_exception {
        CausalRouteReason::SerialInteraction
    } else if strong_evidence_against {
        CausalRouteReason::MatchedEvidenceAgainst
    } else if net_value_lower_bps < 0 {
        CausalRouteReason::NegativeExpectedValue
    } else {
        CausalRouteReason::AdmitPositiveValue
    };
    let admitted = candidate_route != AgentRouteTier::Workflow
        || reason == CausalRouteReason::AdmitPositiveValue;
    let mut actions = vec![candidate_action];
    if let Some(action) = fallback_action {
        actions.push(action);
    }
    let selected_index = usize::from(!admitted);
    let selected_action_id = actions[selected_index].action_id.clone();
    let counterfactual_action_id =
        (actions.len() > 1).then(|| actions[usize::from(admitted)].action_id.clone());
    let receipt = CausalRouteSelectionV2 {
        schema: CAUSAL_ROUTE_SELECTION_SCHEMA_V2.to_string(),
        policy: CAUSAL_ROUTE_SELECTION_POLICY_V2.to_string(),
        feature_snapshot: snapshot.clone(),
        context_fingerprint: snapshot.context_fingerprint.clone(),
        requirements_fingerprint: snapshot.requirements_fingerprint.clone(),
        model_pool_sha256: snapshot.model_pool_sha256.clone(),
        candidate_route,
        selected_route: actions[selected_index].route_tier,
        actions,
        selected_action_id,
        counterfactual_action_id,
        support,
        predicted_benefit_bps,
        evidence_adjusted_benefit_bps,
        coordination_cost_bps,
        latency_cost_bps,
        uncertainty_bps,
        net_value_lower_bps,
        reason,
        deterministic: true,
        propensity_bps: None,
        operations: CausalRouteOperationCounts {
            candidate_evaluations: if candidate_route == AgentRouteTier::Workflow {
                2
            } else {
                1
            },
            evidence_key_lookups,
            historical_rows_scanned: 0,
        },
    };
    receipt.validate()?;
    Ok(receipt)
}

fn route_action(
    decision: &AgentRunDecision,
    model_candidates: &[ModelCandidate],
) -> Result<CausalRouteActionV2, String> {
    let route_tier = decision.route_tier();
    let candidate = model_candidates
        .iter()
        .find(|candidate| candidate.name.trim() == decision.primary_model.trim());
    let tools = candidate
        .map(|candidate| {
            RouteCapabilityEvidence::from_candidate(
                candidate.supports_tools,
                candidate.tools_capability_source,
            )
        })
        .unwrap_or(RouteCapabilityEvidence::Unsupported);
    let vision = candidate
        .map(|candidate| {
            RouteCapabilityEvidence::from_candidate(
                candidate.supports_vision,
                candidate.vision_capability_source,
            )
        })
        .unwrap_or(RouteCapabilityEvidence::Unsupported);
    let projected_model_calls = match route_tier {
        AgentRouteTier::Workflow => {
            decision
                .max_parallelism
                .saturating_add(1)
                .saturating_add(usize::from(
                    decision.verification == AgentVerificationPolicy::Independent,
                ))
        }
        AgentRouteTier::Direct | AgentRouteTier::GroundedDirect => 1,
    }
    .min(8) as u8;
    let cost_tier = candidate
        .map(|candidate| candidate.cost_tier.max(1))
        .unwrap_or(4);
    let latency_tier = candidate
        .map(|candidate| candidate.latency_tier.max(1))
        .unwrap_or(4);
    let critical_stages = match route_tier {
        AgentRouteTier::Workflow => {
            2 + usize::from(decision.verification == AgentVerificationPolicy::Independent)
        }
        AgentRouteTier::Direct | AgentRouteTier::GroundedDirect => 1,
    };
    let feasible = (decision.tool_requirement == AgentToolRequirement::None || tools.usable())
        && (!decision.vision_required || vision.usable());
    let cost_units = u16::from(cost_tier).saturating_mul(u16::from(projected_model_calls));
    let critical_path_latency_units =
        u16::from(latency_tier).saturating_mul(critical_stages as u16);
    Ok(CausalRouteActionV2 {
        action_id: causal_route_action_id_v2(decision)?,
        route_tier,
        model: decision.primary_model.clone(),
        tools,
        vision,
        projected_model_calls,
        cost_units,
        critical_path_latency_units,
        feasible,
    })
}

pub fn causal_route_action_id_v2(decision: &AgentRunDecision) -> Result<String, String> {
    let identity = RouteActionIdentityV2 {
        schema: CAUSAL_ROUTE_SELECTION_SCHEMA_V2,
        route_tier: decision.route_tier(),
        primary_model: decision.primary_model.clone(),
        tool_requirement: decision.tool_requirement,
        vision_required: decision.vision_required,
        risk_level: decision.risk_level,
        retrieval_channels: decision.retrieval.channels.clone(),
        retrieval_max_results: decision.retrieval.max_results,
        retrieval_query_sha256: sha256_hex(decision.retrieval.query.as_bytes()),
        memory_policy: decision.memory.policy,
        memory_query_sha256: sha256_hex(decision.memory.query.as_bytes()),
        verification: decision.verification,
        max_parallelism: decision.max_parallelism,
        min_successful_branches: decision.min_successful_branches,
        distinct_contributions: decision.distinct_contributions,
        estimated_steps: decision.estimated_steps,
        stop_policy: decision.stop_policy,
    };
    serde_json::to_vec(&identity)
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("causal route action identity serialization failed: {error}"))
}

fn route_support(
    snapshot: &RouteFeatureSnapshotV2,
    decision: &AgentRunDecision,
    candidate_action_id: &str,
    matched: Option<&MatchedCollaborationEvidence>,
) -> CausalRouteSupportV2 {
    let Some(evidence) = matched else {
        return CausalRouteSupportV2 {
            basis: CausalRouteEvidenceBasis::RuntimeDemandOnly,
            evidence_sha256: None,
            matched_examples: 0,
            observed_uplift_bps: None,
        };
    };
    let common_scope = evidence.task_class == decision.task_class
        && evidence.effort.eq_ignore_ascii_case(&snapshot.effort)
        && !candidate_action_id.is_empty();
    let exact_context_action = common_scope
        && evidence.pre_decision_context_fingerprint == snapshot.context_fingerprint
        && evidence.route_action_id == candidate_action_id;
    let legacy_action = common_scope
        && evidence.pre_decision_context_fingerprint.is_empty()
        && evidence.route_action_id.is_empty()
        && evidence.routing_signature == decision.learning_signature();
    if !exact_context_action && !legacy_action {
        return CausalRouteSupportV2 {
            basis: CausalRouteEvidenceBasis::RuntimeDemandOnly,
            evidence_sha256: None,
            matched_examples: 0,
            observed_uplift_bps: None,
        };
    }
    let canonical = format!(
        "context={};class={};effort={};action={};legacy_action={};examples={};wins={};below={};uplift={};team_latency={};anchor_latency={}",
        evidence.pre_decision_context_fingerprint,
        evidence.task_class.label(),
        evidence.effort,
        evidence.route_action_id,
        evidence.routing_signature,
        evidence.examples,
        evidence.team_wins,
        evidence.below_admission_floor,
        evidence.average_uplift_bps,
        evidence.average_team_latency_ms,
        evidence.average_anchor_latency_ms.unwrap_or_default(),
    );
    CausalRouteSupportV2 {
        basis: if exact_context_action {
            CausalRouteEvidenceBasis::MatchedContextAction
        } else {
            CausalRouteEvidenceBasis::LegacyMatchedAction
        },
        evidence_sha256: Some(sha256_hex(canonical.as_bytes())),
        matched_examples: evidence.examples,
        observed_uplift_bps: Some(evidence.average_uplift_bps),
    }
}

fn demand_benefit_bps(snapshot: &RouteFeatureSnapshotV2, decision: &AgentRunDecision) -> i32 {
    400i32
        .saturating_add(i32::from(snapshot.parallelizable) * 1_600)
        .saturating_add(i32::from(snapshot.verification_required) * 650)
        .saturating_add(i32::from(snapshot.high_stakes) * 800)
        .saturating_add(i32::from(snapshot.needs_retrieval) * 350)
        .saturating_add(
            i32::from(decision.verification == AgentVerificationPolicy::Independent) * 900,
        )
        .saturating_add(i32::from(snapshot.complexity_band) * 450)
        .saturating_add(i32::from(snapshot.estimated_steps >= 4) * 550)
        .saturating_sub(i32::from(snapshot.latency_sensitive) * 1_000)
        .max(0)
}

fn model_pool_digest(candidates: &[ModelCandidate]) -> String {
    let mut entries = candidates
        .iter()
        .map(|candidate| {
            format!(
                "{}|{:?}|{}|{}|{}|{}|{}|{}",
                candidate.name,
                candidate.role,
                u8::from(candidate.supports_tools),
                candidate.tools_capability_source.label(),
                u8::from(candidate.supports_vision),
                candidate.vision_capability_source.label(),
                candidate.cost_tier,
                candidate.latency_tier,
            )
        })
        .collect::<Vec<_>>();
    entries.sort_unstable();
    entries.dedup();
    sha256_hex(entries.join("\n").as_bytes())
}

fn unique_model_count(candidates: &[ModelCandidate]) -> usize {
    let mut models = candidates
        .iter()
        .map(|candidate| candidate.name.trim())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort_unstable();
    models.dedup();
    models.len()
}

fn normalize_effort(effort: &str) -> String {
    match effort.trim().to_ascii_lowercase().as_str() {
        "fast" => "fast",
        "pro" => "pro",
        _ => "auto",
    }
    .to_string()
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    include!("causal_routing_tests.rs");
}
