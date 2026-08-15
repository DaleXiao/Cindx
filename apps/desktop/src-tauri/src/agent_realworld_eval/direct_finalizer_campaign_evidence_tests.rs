use super::*;
use crate::agent_realworld_eval::direct_finalizer_campaign_contract::{
    DirectFinalizerCallMetrics, DirectFinalizerCampaignCase,
    DirectFinalizerDeterministicVerification, DirectFinalizerPairReceipt,
    DirectFinalizerProviderReceipt, DirectFinalizerReviewerReceipt,
    DIRECT_FINALIZER_GEPA_SUITE_SCHEMA,
};
use orchestrator::ActionableSideInformation;

fn candidate(
    output: &str,
    deterministic_score: f64,
    status: &str,
) -> DirectFinalizerCandidateReceipt {
    DirectFinalizerCandidateReceipt {
        canonical_request_sha256: sha256_hex(format!("request-{output}").as_bytes()),
        directive_sha256: Some(sha256_hex(format!("directive-{output}").as_bytes())),
        output_sha256: sha256_hex(output.as_bytes()),
        deterministic_score,
        provider_receipts: vec![DirectFinalizerProviderReceipt {
            configured_model: "producer".to_string(),
            request_payload_sha256: sha256_hex(format!("payload-{output}").as_bytes()),
            response_semantic_sha256: sha256_hex(output.as_bytes()),
            provider_response_model: Some("producer".to_string()),
            provider_response_id_sha256: None,
            provider_system_fingerprint_sha256: None,
            receipt_status: status.to_string(),
        }],
        metrics: DirectFinalizerCallMetrics {
            latency_ms: 100,
            prompt_tokens: 70,
            completion_tokens: 30,
            total_tokens: 100,
        },
        verification: DirectFinalizerDeterministicVerification {
            passed: true,
            required_groups_passed: 1,
            required_groups_total: 1,
            forbidden_terms_absent: 0,
            forbidden_terms_total: 0,
            exact_json_passed: None,
            failures: Vec::new(),
        },
    }
}

fn reviewer(id: &str) -> DirectFinalizerReviewerReceipt {
    DirectFinalizerReviewerReceipt {
        review_id: id.to_string(),
        reviewer_model: "reviewer".to_string(),
        latency_ms: 20,
        prompt_tokens: 30,
        completion_tokens: 10,
        total_tokens: 40,
        provider_receipts: vec![DirectFinalizerProviderReceipt {
            configured_model: "reviewer".to_string(),
            request_payload_sha256: sha256_hex(format!("review-{id}").as_bytes()),
            response_semantic_sha256: sha256_hex(format!("response-{id}").as_bytes()),
            provider_response_model: Some("reviewer".to_string()),
            provider_response_id_sha256: None,
            provider_system_fingerprint_sha256: None,
            receipt_status: "provider_id_missing".to_string(),
        }],
    }
}

fn case_and_pair(
    index: usize,
    split: DirectFinalizerCaseSplit,
    status: &str,
) -> (
    DirectFinalizerCampaignCase,
    DirectFinalizerCampaignPairEvidence,
) {
    let prefix = match split {
        DirectFinalizerCaseSplit::Train => "train",
        DirectFinalizerCaseSplit::Holdout => "holdout",
    };
    let case_id = format!("{prefix}-{index}");
    let task_class = if index.is_multiple_of(2) {
        "coding"
    } else {
        "research"
    };
    let parent_output = format!("parent output {case_id}");
    let candidate_output = format!("candidate output {case_id}");
    let evaluation_id = format!("evaluation-{case_id}");
    let case = DirectFinalizerCampaignCase {
        id: case_id.clone(),
        split,
        task_class: task_class.to_string(),
        objective: format!("objective {case_id}"),
        evidence_summary: format!("evidence {case_id}"),
        actor_draft: format!("draft {case_id}"),
        required_any_groups: vec![vec!["output".to_string()]],
        forbidden_terms: Vec::new(),
        exact_json: None,
    };
    let mut parent = candidate(&parent_output, 1.0, status);
    parent.directive_sha256 = None;
    let receipt = DirectFinalizerPairReceipt {
        evaluation_id: evaluation_id.clone(),
        case_id,
        split,
        task_class: task_class.to_string(),
        pre_treatment_state_sha256: "1".repeat(64),
        task_contract_sha256: "2".repeat(64),
        parent,
        candidate: candidate(&candidate_output, 1.0, status),
        reviewer_forward: reviewer(&format!("{evaluation_id}-forward")),
        reviewer_reverse: reviewer(&format!("{evaluation_id}-reverse")),
        reviewer_score_parent: 0.7,
        reviewer_score_candidate: 0.9,
        reviewer_safety_violations_parent: 0,
        reviewer_safety_violations_candidate: 0,
        reward_parent: -0.08,
        reward_candidate: 0.08,
    };
    let pair = DirectFinalizerCampaignPairEvidence {
        receipt,
        parent_output,
        candidate_output,
        parent_feedback: ActionableSideInformation {
            summary: "parent feedback".to_string(),
            ..ActionableSideInformation::default()
        },
        candidate_feedback: ActionableSideInformation {
            summary: "candidate feedback".to_string(),
            ..ActionableSideInformation::default()
        },
    };
    (case, pair)
}

fn fixture(
    status: &str,
) -> (
    DirectFinalizerCampaignSuite,
    Vec<DirectFinalizerCampaignPairEvidence>,
) {
    let entries = (0..6)
        .map(|index| case_and_pair(index, DirectFinalizerCaseSplit::Train, status))
        .chain((0..8).map(|index| case_and_pair(index, DirectFinalizerCaseSplit::Holdout, status)))
        .collect::<Vec<_>>();
    let suite = DirectFinalizerCampaignSuite {
        schema: DIRECT_FINALIZER_GEPA_SUITE_SCHEMA.to_string(),
        id: "suite".to_string(),
        version: 1,
        description: "fixture".to_string(),
        gate_a_case_ids: (0..4).map(|index| format!("train-{index}")).collect(),
        cases: entries.iter().map(|(case, _)| case.clone()).collect(),
    };
    (suite, entries.into_iter().map(|(_, pair)| pair).collect())
}

fn profile(id: &str, digest: char) -> DirectFinalizerProfileReceipt {
    DirectFinalizerProfileReceipt {
        profile_id: id.to_string(),
        profile_sha256: digest.to_string().repeat(64),
    }
}

fn gate_config() -> PromptPromotionGateConfig {
    PromptPromotionGateConfig {
        minimum_train_runs: 6,
        minimum_holdout_runs: 8,
        minimum_unique_train_cases: 6,
        minimum_unique_holdout_cases: 8,
        minimum_train_task_classes: 2,
        minimum_holdout_task_classes: 2,
        minimum_wilson_lower_bound: 0.55,
        maximum_generalization_gap: 0.15,
        maximum_holdout_task_class_regression: 0.01,
        maximum_holdout_quality_regression: 0.01,
        maximum_holdout_latency_regression_bps: 500,
        maximum_holdout_token_regression_bps: 200,
    }
}

#[test]
fn projects_train_packets_and_mirrored_scientific_observations() {
    let (suite, pairs) = fixture("provider_id_missing");
    let parent = profile("parent", 'a');
    let candidate = profile("candidate", 'b');
    let model_sha = "c".repeat(64);
    let packets = reflection_packets(&suite, &candidate, &pairs, &model_sha).unwrap();
    assert_eq!(packets.len(), 6);
    assert!(packets.iter().all(|packet| {
        packet.candidate_id == "candidate"
            && packet.final_output.starts_with("candidate output")
            && packet
                .actionable_feedback
                .summary
                .contains("Matched blinded comparison")
            && packet
                .actionable_feedback
                .summary
                .ends_with("candidate feedback")
            && packet
                .model_fingerprints
                .values()
                .all(|value| value == &model_sha)
    }));

    let projection = evaluate_campaign_evidence(
        &suite,
        &parent,
        &candidate,
        &"d".repeat(64),
        &"e".repeat(64),
        "producer",
        "reviewer",
        &pairs,
        gate_config(),
    )
    .unwrap();
    assert_eq!(projection.observations.len(), 28);
    assert!(projection.gate.eligible, "{:?}", projection.gate.blockers);
    assert!(is_sha256(&projection.paired_evidence_sha256));
    assert!(projection
        .observations
        .iter()
        .all(PromptEvolutionObservation::is_strict_matched_evidence));
    let pair = projection
        .observations
        .iter()
        .filter(|observation| observation.case_id == "train-0")
        .collect::<Vec<_>>();
    assert_eq!(pair.len(), 2);
    assert!((pair[0].quality_score - 0.96).abs() < f64::EPSILON * 8.0);
    assert_eq!(pair[0].mode, PromptEvaluationMode::PairedExecution);
    assert_eq!(
        pair[0].provenance.candidate_prompt_sha256,
        pair[1].provenance.opponent_prompt_sha256
    );
    assert_eq!(
        pair[0].provenance.opponent_prompt_sha256,
        pair[1].provenance.candidate_prompt_sha256
    );
}

#[test]
fn rejects_identity_conflict_and_raw_output_tampering() {
    let (suite, mut pairs) = fixture("identity_conflict");
    let error = reflection_packets(&suite, &profile("candidate", 'b'), &pairs, &"c".repeat(64))
        .unwrap_err();
    assert!(error.contains("projection-safe"));

    pairs[0].receipt.parent.provider_receipts[0].receipt_status = "observed".to_string();
    pairs[0].receipt.candidate.provider_receipts[0].receipt_status = "observed".to_string();
    pairs[0].candidate_output = "tampered".to_string();
    let error = match evaluate_campaign_evidence(
        &suite,
        &profile("parent", 'a'),
        &profile("candidate", 'b'),
        &"d".repeat(64),
        &"e".repeat(64),
        "producer",
        "reviewer",
        &pairs,
        gate_config(),
    ) {
        Ok(_) => panic!("tampered evidence unexpectedly projected"),
        Err(error) => error,
    };
    assert!(error.contains("projection-safe"));
}
