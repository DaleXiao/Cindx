use super::requirements::AgentPlanningSource;
use agent_core::Metadata;
use orchestrator::{
    prompt_genome_sha256, select_causal_route_v2, sha256_hex, AgentPolicy, AgentRouteRequirements,
    AgentRunDecision, CausalRouteReason, ConductorPromptGenome, ModelCandidate,
    RouteFeatureSnapshotV2,
};

pub(super) fn neutral_prompt_profile_sha256(effort: AgentPolicy) -> String {
    prompt_genome_sha256(&ConductorPromptGenome::seed_for_effort(effort.label()))
        .expect("built-in route-neutral prompt profile must validate")
}

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
    decision: &mut AgentRunDecision,
) -> Result<(), String> {
    let snapshot = RouteFeatureSnapshotV2::from_request(
        prompt,
        recent_context,
        effort.label(),
        route_requirements,
        candidates,
        budget_fingerprint,
        prompt_profile_sha256,
    );
    let mut receipt = match decision.causal_route.take() {
        Some(receipt) if receipt.feature_snapshot == snapshot => receipt,
        Some(_) => {
            return Err("causal route receipt does not match its planning inputs".to_string())
        }
        None => select_causal_route_v2(decision, &snapshot, candidates, None, 0)?,
    };
    let final_route = decision.route_tier();
    if receipt.selected_route != final_route {
        receipt.reconcile_selected_route(final_route, CausalRouteReason::ExecutionConstraint)?;
    } else if source == AgentPlanningSource::FastDirect {
        receipt.reason = CausalRouteReason::FastPolicy;
    } else if degraded {
        receipt.reason = CausalRouteReason::DegradedFallback;
    }
    receipt.validate()?;
    decision.causal_route = Some(receipt);
    Ok(())
}

pub(super) fn apply_causal_route_to_context(
    decision: &AgentRunDecision,
    run_context: &mut Metadata,
) -> Result<(), String> {
    let receipt = decision
        .causal_route
        .as_ref()
        .ok_or_else(|| "planned run is missing its causal route receipt".to_string())?;
    receipt.validate()?;
    for (key, value) in [
        (
            "pre_decision_context_fingerprint",
            receipt.context_fingerprint.clone(),
        ),
        (
            "route_requirements_fingerprint",
            receipt.requirements_fingerprint.clone(),
        ),
        ("causal_route_policy", receipt.policy.clone()),
        (
            "causal_route_candidate",
            receipt.candidate_route.label().to_string(),
        ),
        (
            "causal_route_selected",
            receipt.selected_route.label().to_string(),
        ),
        (
            "causal_route_selected_action_id",
            receipt.selected_action_id.clone(),
        ),
        ("causal_route_reason", receipt.reason.label().to_string()),
        ("causal_route_receipt_sha256", receipt.digest()?),
    ] {
        run_context.insert(key.to_string(), value);
    }
    Ok(())
}

pub(crate) fn causal_route_event_metadata(
    run_context: &Metadata,
    decision: &AgentRunDecision,
) -> Result<Metadata, String> {
    let receipt = decision
        .causal_route
        .as_ref()
        .ok_or_else(|| "planned run is missing its causal route receipt".to_string())?;
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
