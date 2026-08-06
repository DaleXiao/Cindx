
use super::*;
use crate::{
    matching_collaboration_evidence_for_context, AgentExecutionMode, AgentRiskLevel,
    ConductorStopPolicy, ModelRole,
};

fn candidates() -> Vec<ModelCandidate> {
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

fn requirements() -> AgentRouteRequirements {
    AgentRouteRequirements {
        minimum_tool_requirement: AgentToolRequirement::None,
        effect_authority: AgentEffectAuthority::Forbidden,
        image_input_required: false,
    }
}

fn workflow_decision() -> AgentRunDecision {
    let mut decision = AgentRunDecision::direct("executor");
    decision.task_class = TaskClass::Research;
    decision.execution = AgentExecutionMode::Workflow;
    decision.risk_level = AgentRiskLevel::Elevated;
    decision.verification = AgentVerificationPolicy::Independent;
    decision.max_parallelism = 2;
    decision.min_successful_branches = 2;
    decision.distinct_contributions = 2;
    decision.estimated_steps = 4;
    decision.expected_uplift_bps = 6_000;
    decision.confidence_bps = 8_000;
    decision.stop_policy = ConductorStopPolicy::Quorum;
    decision
}

fn parallel_snapshot() -> RouteFeatureSnapshotV2 {
    RouteFeatureSnapshotV2::from_request(
        "Compare independent architecture alternatives and cross-check sources",
        "",
        "auto",
        requirements(),
        &candidates(),
        Some("1".repeat(64)),
        "2".repeat(64),
    )
}

fn matched_evidence(
    decision: &AgentRunDecision,
    context_fingerprint: String,
    average_uplift_bps: i16,
) -> MatchedCollaborationEvidence {
    MatchedCollaborationEvidence {
        task_class: decision.task_class.clone(),
        effort: "auto".to_string(),
        pre_decision_context_fingerprint: context_fingerprint.clone(),
        route_action_id: if context_fingerprint.is_empty() {
            String::new()
        } else {
            causal_route_action_id_v2(decision).unwrap()
        },
        routing_signature: decision.learning_signature(),
        examples: 8,
        team_wins: 8,
        team_win_rate: 1.0,
        team_win_confidence: 0.67,
        below_admission_floor: 0,
        below_admission_floor_confidence: 0.0,
        anchor_selections: 0,
        average_uplift_bps,
        average_team_latency_ms: 2_000,
        average_anchor_latency_ms: Some(1_500),
    }
}

#[test]
fn feature_identity_binds_exact_inputs_without_persisting_their_text() {
    let build = |objective: &str, recent: &str| {
        RouteFeatureSnapshotV2::from_request(
            objective,
            recent,
            "auto",
            requirements(),
            &candidates(),
            Some("1".repeat(64)),
            "2".repeat(64),
        )
    };
    let first = build("Compare alpha_marker option A", "recent_marker one");
    let objective_changed = build("Compare alpha_marker option B", "recent_marker one");
    let context_changed = build("Compare alpha_marker option A", "recent_marker two");

    assert_ne!(
        first.context_fingerprint,
        objective_changed.context_fingerprint
    );
    assert_ne!(
        first.context_fingerprint,
        context_changed.context_fingerprint
    );
    let encoded = serde_json::to_string(&first).unwrap();
    assert!(!encoded.contains("alpha_marker"));
    assert!(!encoded.contains("recent_marker"));
}

#[test]
fn action_identity_tracks_execution_policy_but_not_predictions_or_rationale() {
    let base = workflow_decision();
    let base_id = causal_route_action_id_v2(&base).unwrap();
    let mut variants = Vec::new();

    let mut tool = base.clone();
    tool.tool_requirement = AgentToolRequirement::ReadOnly;
    variants.push(tool);
    let mut retrieval = base.clone();
    retrieval.retrieval.channels = [WorkspaceRetrievalChannel::Semantic].into_iter().collect();
    retrieval.retrieval.query = "workspace facts".to_string();
    variants.push(retrieval);
    let mut memory = base.clone();
    memory.memory.policy = MemoryRecallPolicy::Relevant;
    memory.memory.query = "prior constraints".to_string();
    variants.push(memory);
    let mut verification = base.clone();
    verification.verification = AgentVerificationPolicy::SelfCheck;
    variants.push(verification);
    let mut quorum = base.clone();
    quorum.min_successful_branches = 1;
    variants.push(quorum);
    let mut stop = base.clone();
    stop.stop_policy = ConductorStopPolicy::Exhaustive;
    variants.push(stop);

    assert!(variants
        .iter()
        .all(|variant| causal_route_action_id_v2(variant).unwrap() != base_id));

    let mut predictions_only = base.clone();
    predictions_only.expected_uplift_bps = 9_999;
    predictions_only.confidence_bps = 1;
    predictions_only.rationale = "different explanation".to_string();
    assert_eq!(
        causal_route_action_id_v2(&predictions_only).unwrap(),
        base_id
    );

    let receipt =
        select_causal_route_v2(&base, &parallel_snapshot(), &candidates(), None, 0).unwrap();
    assert_eq!(receipt.selected_action_id, base_id);
    let fallback_id = causal_route_action_id_v2(&base.constrained_to_grounded_direct()).unwrap();
    assert_eq!(receipt.counterfactual_action_id, Some(fallback_id));
}

#[test]
fn receipt_is_deterministic_bounded_and_keeps_the_counterfactual() {
    let decision = workflow_decision();
    let snapshot = parallel_snapshot();
    let first = select_causal_route_v2(&decision, &snapshot, &candidates(), None, 0)
        .expect("parallel independent work should produce a causal receipt");
    let second = select_causal_route_v2(&decision, &snapshot, &candidates(), None, 0)
        .expect("the same request should replay deterministically");

    assert_eq!(first, second);
    assert_eq!(first.reason, CausalRouteReason::AdmitPositiveValue);
    assert_eq!(first.selected_route, AgentRouteTier::Workflow);
    assert_eq!(first.actions.len(), 2);
    assert!(first.counterfactual_action_id.is_some());
    assert!(first.encoded_len().unwrap() <= CAUSAL_ROUTE_MAX_RECEIPT_BYTES);
    assert_eq!(first.propensity_bps, None);
    first.validate().unwrap();
}

#[test]
fn independent_demand_and_serial_interaction_are_structural_gates() {
    let mut decision = workflow_decision();
    decision.verification = AgentVerificationPolicy::SelfCheck;
    let snapshot = RouteFeatureSnapshotV2::from_request(
        "Explain why the sky is blue",
        "",
        "auto",
        requirements(),
        &candidates(),
        None,
        "2".repeat(64),
    );
    let direct = select_causal_route_v2(&decision, &snapshot, &candidates(), None, 0).unwrap();
    assert_eq!(direct.reason, CausalRouteReason::NoIndependentDemand);
    assert_eq!(direct.selected_route, AgentRouteTier::Direct);
    assert_eq!(
        direct.counterfactual_action_id,
        Some(direct.actions[0].action_id.clone())
    );

    decision.verification = AgentVerificationPolicy::Independent;
    decision.task_class = TaskClass::Browser;
    decision.tool_requirement = AgentToolRequirement::ReadOnly;
    let browser_snapshot = RouteFeatureSnapshotV2::from_request(
        "Use multiple models to open the browser and click the current page",
        "",
        "auto",
        AgentRouteRequirements {
            minimum_tool_requirement: AgentToolRequirement::ReadOnly,
            ..requirements()
        },
        &candidates(),
        None,
        "2".repeat(64),
    );
    let serial =
        select_causal_route_v2(&decision, &browser_snapshot, &candidates(), None, 0).unwrap();
    assert_eq!(serial.reason, CausalRouteReason::SerialInteraction);
    assert_eq!(serial.selected_route, AgentRouteTier::Direct);
}

#[test]
fn only_exact_context_and_action_evidence_adjusts_benefit() {
    let decision = workflow_decision();
    let snapshot = parallel_snapshot();
    let exact = matched_evidence(&decision, snapshot.context_fingerprint.clone(), 300);
    let exact_receipt =
        select_causal_route_v2(&decision, &snapshot, &candidates(), Some(&exact), 1).unwrap();
    assert_eq!(
        exact_receipt.support.basis,
        CausalRouteEvidenceBasis::MatchedContextAction
    );
    assert_eq!(exact_receipt.evidence_adjusted_benefit_bps, 300);

    let legacy = matched_evidence(&decision, String::new(), 300);
    let legacy_receipt =
        select_causal_route_v2(&decision, &snapshot, &candidates(), Some(&legacy), 1).unwrap();
    assert_eq!(
        legacy_receipt.support.basis,
        CausalRouteEvidenceBasis::LegacyMatchedAction
    );
    assert_eq!(
        legacy_receipt.evidence_adjusted_benefit_bps,
        legacy_receipt.predicted_benefit_bps
    );
}

#[test]
fn evidence_matching_is_action_scoped_and_hard_bounded() {
    let decision = workflow_decision();
    let snapshot = parallel_snapshot();
    let mut rows = (0..32)
        .map(|index| {
            let mut row = matched_evidence(&decision, format!("{index:064x}"), 500);
            row.route_action_id = format!("{:064x}", index + 100);
            row
        })
        .collect::<Vec<_>>();
    rows[31] = matched_evidence(&decision, snapshot.context_fingerprint.clone(), 500);
    let teacher = MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(rows.clone());
    let action_id = causal_route_action_id_v2(&decision).unwrap();

    let (matched, lookups) = matching_collaboration_evidence_for_context(
        &decision,
        "auto",
        &snapshot.context_fingerprint,
        &action_id,
        &teacher,
    );
    assert!(matched.is_some());
    assert_eq!(lookups, 1);
    let receipt =
        select_causal_route_v2(&decision, &snapshot, &candidates(), matched, lookups).unwrap();
    assert_eq!(receipt.operations.historical_rows_scanned, 0);

    rows[31].route_action_id = "f".repeat(64);
    let teacher = MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(rows);
    let (unmatched, lookups) = matching_collaboration_evidence_for_context(
        &decision,
        "auto",
        &snapshot.context_fingerprint,
        &action_id,
        &teacher,
    );
    assert!(unmatched.is_none());
    assert_eq!(lookups, 2);

    let wrong_context = matched_evidence(&decision, "e".repeat(64), -500);
    let teacher =
        MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(vec![wrong_context]);
    assert!(matching_collaboration_evidence_for_context(
        &decision,
        "auto",
        &snapshot.context_fingerprint,
        &action_id,
        &teacher,
    )
    .0
    .is_none());
}

#[test]
fn causal_router_v2_contract_gate() {
    let decision = workflow_decision();
    let snapshot = parallel_snapshot();
    let receipt = select_causal_route_v2(&decision, &snapshot, &candidates(), None, 0)
        .expect("causal router should admit independent positive-value work");
    assert_eq!(receipt.schema, CAUSAL_ROUTE_SELECTION_SCHEMA_V2);
    assert_eq!(receipt.policy, CAUSAL_ROUTE_SELECTION_POLICY_V2);
    assert_eq!(receipt.reason, CausalRouteReason::AdmitPositiveValue);
    assert_eq!(receipt.selected_route, AgentRouteTier::Workflow);
    assert_eq!(receipt.actions.len(), 2);
    assert!(receipt.counterfactual_action_id.is_some());
    assert_eq!(receipt.propensity_bps, None);
    assert!(receipt.encoded_len().unwrap() <= CAUSAL_ROUTE_MAX_RECEIPT_BYTES);

    let mut non_independent = decision.clone();
    non_independent.verification = AgentVerificationPolicy::SelfCheck;
    let direct_snapshot = RouteFeatureSnapshotV2::from_request(
        "Explain why the sky is blue",
        "",
        "pro",
        requirements(),
        &candidates(),
        None,
        "2".repeat(64),
    );
    let direct =
        select_causal_route_v2(&non_independent, &direct_snapshot, &candidates(), None, 0).unwrap();
    assert_eq!(direct.reason, CausalRouteReason::NoIndependentDemand);
    assert_eq!(direct.selected_route, AgentRouteTier::Direct);

    let exact = matched_evidence(&decision, snapshot.context_fingerprint.clone(), 250);
    let exact_receipt =
        select_causal_route_v2(&decision, &snapshot, &candidates(), Some(&exact), 1).unwrap();
    assert_eq!(
        exact_receipt.support.basis,
        CausalRouteEvidenceBasis::MatchedContextAction
    );
    assert_eq!(exact_receipt.evidence_adjusted_benefit_bps, 250);
    exact_receipt.validate().unwrap();

    println!(
            "{{\"schema\":\"cindx.causal-router-v2-contract.v1\",\"receipt_schema\":\"{}\",\"actions\":{},\"counterfactual\":true,\"propensity\":null}}",
            receipt.schema,
            receipt.actions.len(),
        );
}

#[test]
fn causal_router_v2_scaling_gate() {
    let decision = workflow_decision();
    let snapshot = parallel_snapshot();
    let rows = |count: usize| {
        (0..count)
            .map(|index| {
                if index + 1 == count {
                    matched_evidence(&decision, snapshot.context_fingerprint.clone(), 500)
                } else {
                    let mut row = matched_evidence(&decision, format!("{index:064x}"), 500);
                    row.route_action_id = format!("{:064x}", index + 10_000);
                    row
                }
            })
            .collect::<Vec<_>>()
    };
    let small = rows(32);
    let large = rows(2_048);
    let action_id = causal_route_action_id_v2(&decision).unwrap();

    let route = |rows: &[MatchedCollaborationEvidence]| {
        let teacher = MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(rows.to_vec());
        let (matched, lookups) = matching_collaboration_evidence_for_context(
            &decision,
            "auto",
            &snapshot.context_fingerprint,
            &action_id,
            &teacher,
        );
        select_causal_route_v2(&decision, &snapshot, &candidates(), matched, lookups).unwrap()
    };
    let small_receipt = route(&small);
    let large_receipt = route(&large);

    assert_eq!(small_receipt, large_receipt);
    assert_eq!(small_receipt.operations.historical_rows_scanned, 0);
    assert_eq!(small_receipt.operations.candidate_evaluations, 2);
    assert_eq!(small_receipt.operations.evidence_key_lookups, 1);
    assert!(small_receipt.encoded_len().unwrap() <= CAUSAL_ROUTE_MAX_RECEIPT_BYTES);
    println!(
            "{{\"schema\":\"cindx.causal-router-v2-scaling.v1\",\"small_rows\":32,\"large_rows\":2048,\"historical_rows_scanned\":{},\"key_lookups\":{},\"candidate_evaluations\":{},\"receipt_bytes\":{}}}",
            small_receipt.operations.historical_rows_scanned,
            small_receipt.operations.evidence_key_lookups,
            small_receipt.operations.candidate_evaluations,
            small_receipt.encoded_len().unwrap(),
        );
}
