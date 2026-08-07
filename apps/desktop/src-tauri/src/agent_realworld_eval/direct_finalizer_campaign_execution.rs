use super::direct_finalizer::{
    prepare_direct_finalizer_pair, DirectFinalizerEvalConfig, PreparedDirectFinalizerArm,
};
use super::direct_finalizer_campaign_contract::*;
use super::direct_finalizer_campaign_support::{
    reserve_provider_calls, write_checkpoint, DirectFinalizerCampaignPairEvidence,
    PrivateCampaignCheckpoint,
};
use super::receipts::{model_receipts_from_metadata, ModelReceipt};
use crate::agent_run_engine::build_agent_finalizer_provider;
use crate::collaboration_execution::complete_collaboration_model_with_control;
use crate::runtime_values::collaboration_system_prompt_for_run;
use agent_core::{Message, MessageRole, Metadata, ModelRole, TaskId, ToolOutcomeStatus, ToolRisk};
use agent_runtime::{
    bounded_max_output_tokens, start_agent_loop, AgentLoopState, AgentRunControl,
    AgentRuntimeConfig,
};
use model_provider::{ModelProvider, ModelResponse};
use orchestrator::{sha256_hex, ConductorPromptGenome};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

const RUNTIME_CONTEXT: &str =
    "Frozen matched Direct-finalizer evaluation. Treat the tool result as the complete evidence boundary.";

struct ArmExecution {
    output: String,
    verification: DirectFinalizerDeterministicVerification,
    receipts: Vec<DirectFinalizerProviderReceipt>,
    metrics: DirectFinalizerCallMetrics,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ensure_pair(
    checkpoint: &mut PrivateCampaignCheckpoint,
    checkpoint_path: &Path,
    case: &DirectFinalizerCampaignCase,
    provider: &crate::configuration_models::ProviderConfig,
    producer_model: &str,
    reviewer_model: &str,
    parent: &ConductorPromptGenome,
    candidate: &ConductorPromptGenome,
) -> Result<(), String> {
    if checkpoint
        .pairs
        .iter()
        .any(|pair| pair.receipt.case_id == case.id)
    {
        return Ok(());
    }
    eprintln!(
        "[direct-finalizer-gepa] case={} split={:?}",
        case.id, case.split
    );
    // Reserve and durably checkpoint the whole matched pair before any external
    // request. A crash or provider failure can consume budget, but can never
    // make the next process forget calls that may already have been sent.
    reserve_provider_calls(checkpoint, checkpoint_path, 4)?;
    let pair = execute_pair(
        case,
        provider,
        producer_model,
        reviewer_model,
        parent,
        candidate,
    )?;
    checkpoint.pairs.push(pair);
    checkpoint
        .pairs
        .sort_by(|left, right| left.receipt.case_id.cmp(&right.receipt.case_id));
    write_checkpoint(checkpoint_path, checkpoint)
}

#[allow(clippy::too_many_arguments)]
fn execute_pair(
    case: &DirectFinalizerCampaignCase,
    provider_config: &crate::configuration_models::ProviderConfig,
    producer_model: &str,
    reviewer_model: &str,
    parent: &ConductorPromptGenome,
    candidate: &ConductorPromptGenome,
) -> Result<DirectFinalizerCampaignPairEvidence, String> {
    let evaluation_id = format!("{}:{}", DIRECT_FINALIZER_GEPA_SUITE_ID, case.id);
    let runtime = frozen_case_runtime(case)?;
    let config = DirectFinalizerEvalConfig {
        provider_id: provider_config.provider_id.clone(),
        model_id: producer_model.to_string(),
        system_prompt: Some(provider_config.agent_system_prompt.clone()),
        runtime_context: Some(RUNTIME_CONTEXT.to_string()),
        context_window_tokens: provider_config.context_window_tokens,
        max_output_tokens: bounded_max_output_tokens(
            provider_config.context_window_tokens,
            crate::runtime_constants::AGENT_MAX_OUTPUT_TOKENS,
        ),
    };
    let pair = prepare_direct_finalizer_pair(
        runtime,
        config,
        parent.direct_finalizer_phenotype().directive(),
        candidate.direct_finalizer_phenotype().directive(),
    )?;
    let parent_receipt = pair.parent.receipt.clone();
    let candidate_receipt = pair.challenger.receipt.clone();
    let parent_control = AgentRunControl::new("auto");
    let candidate_control = AgentRunControl::new("auto");
    let parent_provider =
        build_agent_finalizer_provider(provider_config, producer_model, &parent_control);
    let candidate_provider =
        build_agent_finalizer_provider(provider_config, producer_model, &candidate_control);
    let parent_request = pair.parent.request().clone();
    let candidate_request = pair.challenger.request().clone();
    let (parent_response, candidate_response) = std::thread::scope(|scope| {
        let parent = scope.spawn(move || {
            let started = Instant::now();
            (
                parent_provider.complete(parent_request),
                elapsed_ms(started),
            )
        });
        let candidate = scope.spawn(move || {
            let started = Instant::now();
            (
                candidate_provider.complete(candidate_request),
                elapsed_ms(started),
            )
        });
        (
            parent
                .join()
                .map_err(|_| "parent Direct-finalizer provider panicked".to_string()),
            candidate
                .join()
                .map_err(|_| "candidate Direct-finalizer provider panicked".to_string()),
        )
    });
    let (parent_response, parent_latency_ms) = parent_response?;
    let parent_response = parent_response
        .map_err(|error| format!("parent Direct-finalizer provider failed: {error}"))?;
    let (candidate_response, candidate_latency_ms) = candidate_response?;
    let candidate_response = candidate_response
        .map_err(|error| format!("candidate Direct-finalizer provider failed: {error}"))?;
    let parent_arm = finish_arm(case, pair.parent, parent_response, parent_latency_ms)?;
    let candidate_arm = finish_arm(
        case,
        pair.challenger,
        candidate_response,
        candidate_latency_ms,
    )?;

    let reviewer_case =
        super::direct_finalizer_evaluation_feedback::DirectFinalizerEvaluationCase {
            objective: case.objective.clone(),
            evidence_summary: case.evidence_summary.clone(),
        };
    let parent_for_review =
        super::direct_finalizer_evaluation_feedback::DirectFinalizerEvaluationCandidate {
            producer_model: producer_model.to_string(),
            output: parent_arm.output.clone(),
            deterministic_verifier_summary: verification_summary(&parent_arm.verification),
        };
    let candidate_for_review =
        super::direct_finalizer_evaluation_feedback::DirectFinalizerEvaluationCandidate {
            producer_model: producer_model.to_string(),
            output: candidate_arm.output.clone(),
            deterministic_verifier_summary: verification_summary(&candidate_arm.verification),
        };
    let forward_control = Arc::new(AgentRunControl::new("auto"));
    let reverse_control = Arc::new(AgentRunControl::new("auto"));
    let review = super::direct_finalizer_evaluation_feedback::evaluate_direct_finalizer_pair_position_balanced(
        provider_config,
        reviewer_model,
        &reviewer_case,
        &parent_for_review,
        &candidate_for_review,
        &evaluation_id,
        &forward_control,
        &reverse_control,
    )?;
    let parent_score = combined_quality(
        deterministic_score(&parent_arm.verification),
        review.payload.score_a,
    );
    let candidate_score = combined_quality(
        deterministic_score(&candidate_arm.verification),
        review.payload.score_b,
    );
    let receipt = DirectFinalizerPairReceipt {
        evaluation_id,
        case_id: case.id.clone(),
        split: case.split,
        task_class: case.task_class.clone(),
        pre_treatment_state_sha256: parent_receipt.pre_treatment_sha256.clone(),
        task_contract_sha256: parent_receipt.task_contract_sha256.clone(),
        parent: arm_receipt(parent_receipt, &parent_arm),
        candidate: arm_receipt(candidate_receipt, &candidate_arm),
        reviewer_forward: reviewer_receipt(review.forward),
        reviewer_reverse: reviewer_receipt(review.reverse),
        reviewer_score_parent: review.payload.score_a,
        reviewer_score_candidate: review.payload.score_b,
        reviewer_safety_violations_parent: review.payload.safety_violations_a,
        reviewer_safety_violations_candidate: review.payload.safety_violations_b,
        reward_parent: (parent_score - candidate_score).clamp(-1.0, 1.0),
        reward_candidate: (candidate_score - parent_score).clamp(-1.0, 1.0),
    };
    Ok(DirectFinalizerCampaignPairEvidence {
        receipt,
        parent_output: parent_arm.output,
        candidate_output: candidate_arm.output,
        parent_feedback: review.payload.feedback_a,
        candidate_feedback: review.payload.feedback_b,
    })
}

fn finish_arm(
    case: &DirectFinalizerCampaignCase,
    arm: PreparedDirectFinalizerArm,
    response: ModelResponse,
    latency_ms: u64,
) -> Result<ArmExecution, String> {
    let receipts = model_receipts_from_metadata(&response.metadata)?
        .into_iter()
        .map(provider_receipt)
        .collect();
    let metrics = call_metrics(&response.metadata, latency_ms);
    let resolution = arm
        .resolve(response)
        .map_err(|error| format!("Direct-finalizer resolution failed: {error}"))?;
    if resolution.used_fallback {
        return Err("Direct-finalizer campaign forbids fallback output".to_string());
    }
    let output = resolution.candidate.content;
    let verification = verify_direct_finalizer_output(case, &output);
    Ok(ArmExecution {
        output,
        verification,
        receipts,
        metrics,
    })
}

fn frozen_case_runtime(case: &DirectFinalizerCampaignCase) -> Result<AgentLoopState, String> {
    let task_id = TaskId(format!("direct-finalizer-gepa:{}", case.id));
    let mut runtime = start_agent_loop(
        task_id,
        case.objective.clone(),
        AgentRuntimeConfig::default(),
    );
    let call_id = format!("frozen-evidence-{}", case.id);
    let input_json = serde_json::to_string(&serde_json::json!({
        "case_id": case.id,
        "evidence": case.evidence_summary,
    }))
    .map_err(|error| format!("failed to encode frozen evidence: {error}"))?;
    runtime.task_contract.record_tool_outcome(
        "evaluation.frozen_evidence",
        &input_json,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::ReadOnly),
    );
    let evidence_sequence = runtime
        .task_contract
        .evidence()
        .last()
        .map(|evidence| evidence.sequence)
        .ok_or_else(|| "frozen evidence did not enter the task contract".to_string())?;
    runtime.messages.push(Message {
        role: MessageRole::Assistant,
        content: String::new(),
        metadata: [
            ("tool_call_count".to_string(), "1".to_string()),
            ("tool_call_ids".to_string(), call_id.clone()),
            (
                "raw_tool_calls_json".to_string(),
                serde_json::json!([{
                    "id": call_id,
                    "type": "function",
                    "function": {"name": "evaluation.frozen_evidence", "arguments": "{}"}
                }])
                .to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    runtime.messages.push(Message {
        role: MessageRole::Tool,
        content: case.evidence_summary.clone(),
        metadata: [
            ("kind".to_string(), "tool_observation".to_string()),
            ("tool_call_id".to_string(), call_id),
            (
                "contract_evidence_sequence".to_string(),
                evidence_sequence.to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    runtime.messages.push(Message {
        role: MessageRole::Assistant,
        content: case.actor_draft.clone(),
        metadata: [("kind".to_string(), "actor_draft".to_string())]
            .into_iter()
            .collect(),
    });
    runtime.turn = 2;
    Ok(runtime)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn evaluate_candidate(
    provider: &crate::configuration_models::ProviderConfig,
    gepa_model: &str,
    parent: &ConductorPromptGenome,
    manual_candidate: &ConductorPromptGenome,
    packets: &[orchestrator::AgentEvaluationReflectionPacket],
    checkpoint: &mut PrivateCampaignCheckpoint,
    checkpoint_path: &Path,
) -> Result<(orchestrator::DirectFinalizerGepaDecision, String, u32), String> {
    let (mut response, mut attempts) = match checkpoint.gepa_response.clone() {
        Some(response) => (response, checkpoint.gepa_attempts),
        None => {
            let prompt =
                parent.direct_finalizer_candidate_decision_prompt(manual_candidate, packets)?;
            reserve_provider_calls(checkpoint, checkpoint_path, 1)?;
            let response = run_gepa_call(provider, gepa_model, prompt)?;
            checkpoint.gepa_response = Some(response.clone());
            checkpoint.gepa_attempts = 1;
            write_checkpoint(checkpoint_path, checkpoint)?;
            (response, 1)
        }
    };

    loop {
        match parent.direct_finalizer_decision_from_response(manual_candidate, &response) {
            Ok(decision) => {
                return Ok((decision, sha256_hex(response.as_bytes()), attempts));
            }
            Err(error) if attempts < 2 => {
                let repair = parent.direct_finalizer_candidate_decision_repair_prompt(
                    manual_candidate,
                    &response,
                    &error,
                )?;
                reserve_provider_calls(checkpoint, checkpoint_path, 1)?;
                response = run_gepa_call(provider, gepa_model, repair)?;
                attempts += 1;
                checkpoint.gepa_response = Some(response.clone());
                checkpoint.gepa_attempts = attempts;
                write_checkpoint(checkpoint_path, checkpoint)?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn run_gepa_call(
    provider: &crate::configuration_models::ProviderConfig,
    gepa_model: &str,
    prompt: String,
) -> Result<String, String> {
    let control = Arc::new(AgentRunControl::new("pro"));
    let completion = complete_collaboration_model_with_control(
        provider.clone(),
        ModelRole::Planner,
        gepa_model.to_string(),
        collaboration_system_prompt_for_run(&provider.agent_system_prompt, &Metadata::new()),
        prompt,
        Some(control),
        |_| {},
    );
    completion.content.ok_or_else(|| {
        completion
            .error
            .unwrap_or_else(|| "Direct-finalizer GEPA call returned no content".to_string())
    })
}

fn arm_receipt(
    identity: super::direct_finalizer::DirectFinalizerArmReceipt,
    execution: &ArmExecution,
) -> DirectFinalizerCandidateReceipt {
    DirectFinalizerCandidateReceipt {
        canonical_request_sha256: identity.canonical_request_sha256,
        directive_sha256: identity.directive_sha256,
        output_sha256: sha256_hex(execution.output.as_bytes()),
        deterministic_score: deterministic_score(&execution.verification),
        provider_receipts: execution.receipts.clone(),
        metrics: execution.metrics.clone(),
        verification: execution.verification.clone(),
    }
}

fn provider_receipt(receipt: ModelReceipt) -> DirectFinalizerProviderReceipt {
    DirectFinalizerProviderReceipt {
        configured_model: receipt.configured_model,
        request_payload_sha256: receipt.request_payload_sha256,
        response_semantic_sha256: receipt.response_semantic_sha256,
        provider_response_model: receipt.provider_response_model,
        provider_response_id_sha256: receipt.provider_response_id_sha256,
        provider_system_fingerprint_sha256: receipt.provider_system_fingerprint_sha256,
        receipt_status: receipt.receipt_status,
    }
}

fn reviewer_receipt(
    receipt: super::direct_finalizer_evaluation_feedback::DirectFinalizerReviewerCallReceipt,
) -> DirectFinalizerReviewerReceipt {
    DirectFinalizerReviewerReceipt {
        review_id: receipt.review_id,
        reviewer_model: receipt.reviewer_model,
        latency_ms: receipt.latency_ms,
        prompt_tokens: super::metadata_u64(&receipt.usage, "prompt_tokens"),
        completion_tokens: super::metadata_u64(&receipt.usage, "completion_tokens"),
        total_tokens: total_tokens(&receipt.usage),
        provider_receipts: receipt
            .provider_receipts
            .into_iter()
            .map(provider_receipt)
            .collect(),
    }
}

fn call_metrics(metadata: &Metadata, latency_ms: u64) -> DirectFinalizerCallMetrics {
    DirectFinalizerCallMetrics {
        latency_ms,
        prompt_tokens: super::metadata_u64(metadata, "prompt_tokens"),
        completion_tokens: super::metadata_u64(metadata, "completion_tokens"),
        total_tokens: total_tokens(metadata),
    }
}

fn total_tokens(metadata: &Metadata) -> u64 {
    let total = super::metadata_u64(metadata, "total_tokens");
    if total > 0 {
        total
    } else {
        super::metadata_u64(metadata, "prompt_tokens")
            .saturating_add(super::metadata_u64(metadata, "completion_tokens"))
    }
}

fn deterministic_score(verification: &DirectFinalizerDeterministicVerification) -> f64 {
    let exact_total = usize::from(verification.exact_json_passed.is_some());
    let exact_passed = usize::from(verification.exact_json_passed == Some(true));
    let passed = verification
        .required_groups_passed
        .saturating_add(verification.forbidden_terms_absent)
        .saturating_add(exact_passed);
    let total = verification
        .required_groups_total
        .saturating_add(verification.forbidden_terms_total)
        .saturating_add(exact_total)
        .max(1);
    passed as f64 / total as f64
}

fn combined_quality(deterministic: f64, reviewer: f64) -> f64 {
    (deterministic.clamp(0.0, 1.0) * 0.6 + reviewer.clamp(0.0, 1.0) * 0.4).clamp(0.0, 1.0)
}

fn verification_summary(verification: &DirectFinalizerDeterministicVerification) -> String {
    format!(
        "passed={} score={:.3} failures={:?}",
        verification.passed,
        deterministic_score(verification),
        verification.failures
    )
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}
