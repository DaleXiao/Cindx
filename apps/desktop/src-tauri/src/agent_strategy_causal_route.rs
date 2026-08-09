use super::requirements::AgentPlanningSource;
use agent_core::Metadata;
use orchestrator::{
    causal_route_action_id_v2, select_causal_route_v2, sha256_hex, AgentPolicy,
    AgentRouteRequirements, AgentRunDecision, CausalRouteReason, CausalRouteSelectionV2,
    ConductorPromptGenome, ExecutionPlan, ModelCandidate, RouteFeatureRequest,
    RouteFeatureSnapshotV2, RoutingContext,
};

pub(super) fn run_decision_evolved_directive(
    profile: &ConductorPromptGenome,
    effort: AgentPolicy,
) -> Result<String, String> {
    profile
        .route_decision_directive(effort.label())
        .map(|directive| directive.unwrap_or_default())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_causal_route(
    prompt: &str,
    recent_context: &str,
    effort: AgentPolicy,
    route_requirements: AgentRouteRequirements,
    candidates: &[ModelCandidate],
    budget_fingerprint: Option<String>,
    prompt_profile_sha256: String,
    source: AgentPlanningSource,
    degraded: bool,
    conductor_candidate: &AgentRunDecision,
    compatibility_route: Option<CausalRouteSelectionV2>,
) -> Result<CausalRouteSelectionV2, String> {
    let snapshot = RouteFeatureSnapshotV2::from_decision_request(
        conductor_candidate,
        RouteFeatureRequest {
            objective: prompt,
            recent_context,
            effort: effort.label(),
            requirements: route_requirements,
            budget_fingerprint: budget_fingerprint.as_deref(),
            prompt_profile_sha256: &prompt_profile_sha256,
        },
        candidates,
    );
    let mut receipt = match compatibility_route {
        Some(receipt) if receipt.feature_snapshot == snapshot => receipt,
        Some(_) => {
            return Err("causal route receipt does not match its planning inputs".to_string())
        }
        None => select_causal_route_v2(conductor_candidate, &snapshot, candidates, None, 0)?,
    };
    if source == AgentPlanningSource::FastDirect {
        receipt.reason = CausalRouteReason::FastPolicy;
    } else if degraded {
        receipt.reason = CausalRouteReason::DegradedFallback;
    }
    receipt.validate()?;
    Ok(receipt)
}

pub(super) fn apply_causal_route_to_context(
    plan: &ExecutionPlan,
    run_context: &mut Metadata,
) -> Result<(), String> {
    let receipt = &plan.compatibility_route;
    receipt.validate()?;
    let executable_action_id = causal_route_action_id_v2(plan.action())?;
    let pre_decision_task_class = run_context
        .get("effective_prompt_objective")
        .or_else(|| run_context.get("prompt_objective"))
        .map(|objective| RoutingContext::from_prompt(objective, Vec::new()).task_class)
        .unwrap_or_else(|| receipt.feature_snapshot.task_class.clone());
    for (key, value) in [
        (
            "pre_decision_context_fingerprint",
            receipt.context_fingerprint.clone(),
        ),
        (
            "pre_decision_task_class",
            pre_decision_task_class.label().to_string(),
        ),
        (
            "route_requirements_fingerprint",
            receipt.requirements_fingerprint.clone(),
        ),
        ("causal_route_policy", receipt.policy.clone()),
        (
            "route_prompt_profile_sha256",
            receipt.feature_snapshot.prompt_profile_sha256.clone(),
        ),
        (
            "causal_route_candidate",
            receipt.candidate_route.label().to_string(),
        ),
        (
            "causal_route_selected",
            plan.action().route_tier().label().to_string(),
        ),
        (
            "causal_route_selected_action_id",
            executable_action_id,
        ),
        (
            "causal_route_shadow_selected",
            receipt.selected_route.label().to_string(),
        ),
        (
            "causal_route_shadow_selected_action_id",
            receipt.selected_action_id.clone(),
        ),
        ("causal_route_reason", receipt.reason.label().to_string()),
        (
            "causal_route_shadow_reason",
            receipt.reason.label().to_string(),
        ),
        ("causal_route_receipt_sha256", receipt.digest()?),
    ] {
        run_context.insert(key.to_string(), value);
    }
    Ok(())
}

pub(crate) fn causal_route_event_metadata(
    run_context: &Metadata,
    plan: &ExecutionPlan,
) -> Result<Metadata, String> {
    let receipt = &plan.compatibility_route;
    receipt.validate()?;
    let receipt_json = serde_json::to_string(receipt)
        .map_err(|error| format!("causal route receipt serialization failed: {error}"))?;
    let receipt_sha256 = receipt.digest()?;
    let route_decision_id = sha256_hex(
        format!(
            "run={};epoch={};receipt={receipt_sha256}",
            run_context
                .get("agent_run_id")
                .map(String::as_str)
                .unwrap_or("unknown"),
            run_context
                .get("steer_epoch")
                .map(String::as_str)
                .unwrap_or("0"),
        )
        .as_bytes(),
    );
    Ok([
        ("route_decision_id".to_string(), route_decision_id),
        ("causal_route_receipt".to_string(), receipt_json),
        ("causal_route_receipt_sha256".to_string(), receipt_sha256),
    ]
    .into_iter()
    .collect())
}
