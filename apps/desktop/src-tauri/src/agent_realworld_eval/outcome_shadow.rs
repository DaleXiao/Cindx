use super::receipts::TreatmentExposureReceipt;
use super::RawRun;
use agent_application::{
    AgentExternalPostconditionV1, AgentExternalVerifierV1, AgentOutcomeExposureV1,
    AgentOutcomeLifecycleBindingV1, AgentOutcomeResourcesV1, AgentOutcomeTerminalResourcesV1,
    AgentOutcomeTerminalStatusV1, ExternallyVerifiedOutcomeV1,
};
use agent_core::Event;
use orchestrator::sha256_hex;
use serde::Serialize;

const REALWORLD_VERIFIER_KIND: &str = "cindx.agent-realworld-postconditions.v1";
const BUDGET_RECEIPT_DOMAIN: &[u8] = b"cindx.agent-realworld-outcome-budget.v1\0";
const MODEL_RECEIPTS_DOMAIN: &[u8] = b"cindx.agent-realworld-outcome-models.v1\0";
const TOOL_RECEIPTS_DOMAIN: &[u8] = b"cindx.agent-realworld-outcome-tools.v1\0";
const VERIFIER_PROTOCOL_DOMAIN: &[u8] = b"cindx.agent-realworld-verifier-protocol.v1\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ShadowOutcomeTraceV1 {
    pub(super) lifecycle: AgentOutcomeLifecycleBindingV1,
    pub(super) exposure: AgentOutcomeExposureV1,
    pub(super) terminal_resources: AgentOutcomeTerminalResourcesV1,
}

impl ShadowOutcomeTraceV1 {
    pub(super) fn from_events(events: &[Event]) -> Result<Self, String> {
        let lifecycle = AgentOutcomeLifecycleBindingV1::from_events(events)
            .map_err(|error| error.to_string())?;
        let exposure = AgentOutcomeExposureV1::from_events(events, &lifecycle)
            .map_err(|error| error.to_string())?;
        let terminal_resources = AgentOutcomeTerminalResourcesV1::from_events(events, &lifecycle)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            lifecycle,
            exposure,
            terminal_resources,
        })
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct ShadowOutcomePairV1 {
    pub(super) direct: ExternallyVerifiedOutcomeV1,
    pub(super) workflow: ExternallyVerifiedOutcomeV1,
}

impl ShadowOutcomePairV1 {
    pub(super) fn new(
        direct: ExternallyVerifiedOutcomeV1,
        workflow: ExternallyVerifiedOutcomeV1,
    ) -> Result<Self, String> {
        direct.validate().map_err(|error| error.to_string())?;
        workflow.validate().map_err(|error| error.to_string())?;
        if direct.verifier.kind != workflow.verifier.kind
            || direct.verifier.subject_sha256 != workflow.verifier.subject_sha256
            || direct.verifier.protocol_sha256 != workflow.verifier.protocol_sha256
        {
            return Err(
                "matched route shadow outcomes do not share one external verifier".to_string(),
            );
        }
        if direct.resources.budget_sha256 != workflow.resources.budget_sha256 {
            return Err("matched route shadow outcomes do not share one budget".to_string());
        }
        Ok(Self { direct, workflow })
    }
}

pub(super) fn project_shadow_outcome_pair(
    direct: &RawRun,
    workflow: &RawRun,
) -> (Option<ShadowOutcomePairV1>, Option<String>) {
    let projected = project_outcome(direct).and_then(|direct| {
        project_outcome(workflow).and_then(|workflow| ShadowOutcomePairV1::new(direct, workflow))
    });
    match projected {
        Ok(pair) => (Some(pair), None),
        Err(error) => (None, Some(error)),
    }
}

pub(super) fn project_outcome(run: &RawRun) -> Result<ExternallyVerifiedOutcomeV1, String> {
    let trace = run.outcome_trace.as_ref().ok_or_else(|| {
        run.outcome_trace_error
            .clone()
            .unwrap_or_else(|| "externally verified outcome trace is missing".to_string())
    })?;
    validate_trace_consistency(run, trace)?;

    let postconditions = run
        .verification
        .postcondition_receipts
        .iter()
        .map(|receipt| AgentExternalPostconditionV1 {
            kind: receipt.kind.clone(),
            subject_sha256: receipt.subject_sha256.clone(),
            expected_sha256: receipt.expected_sha256.clone(),
            observed_sha256: receipt.observed_sha256.clone(),
            artifact_sha256: receipt.artifact_sha256.clone(),
            bytes: receipt.bytes,
            passed: receipt.passed,
            preservation: receipt.kind == "immutable_fixture",
        })
        .collect::<Vec<_>>();
    let verifier = AgentExternalVerifierV1 {
        kind: REALWORLD_VERIFIER_KIND.to_string(),
        protocol_sha256: verifier_protocol_sha256(&postconditions)?,
        subject_sha256: run.input_sha256.clone(),
        safety_violations: run.verification.safety_violations,
    };
    let resources = AgentOutcomeResourcesV1::new(
        receipt_sha256(BUDGET_RECEIPT_DOMAIN, &run.resolved_budget)?,
        receipt_sha256(MODEL_RECEIPTS_DOMAIN, &run.model_receipts)?,
        receipt_sha256(TOOL_RECEIPTS_DOMAIN, &run.tool_receipts)?,
        run.metrics.latency_ms,
        run.metrics.model_calls,
        run.metrics.tool_calls,
        trace.terminal_resources.clone(),
    )
    .map_err(|error| error.to_string())?;
    ExternallyVerifiedOutcomeV1::new(
        trace.lifecycle.clone(),
        trace.exposure.clone(),
        verifier,
        postconditions,
        resources,
    )
    .map_err(|error| error.to_string())
}

fn validate_trace_consistency(run: &RawRun, trace: &ShadowOutcomeTraceV1) -> Result<(), String> {
    let completed = trace.lifecycle.terminal_status == AgentOutcomeTerminalStatusV1::Completed;
    let terminal_status = match trace.lifecycle.terminal_status {
        AgentOutcomeTerminalStatusV1::Completed => "completed",
        AgentOutcomeTerminalStatusV1::Failed => "failed",
        AgentOutcomeTerminalStatusV1::Cancelled => "cancelled",
    };
    if run.completed != completed || run.terminal_status != terminal_status {
        return Err("externally verified outcome terminal metrics disagree with trace".to_string());
    }
    if run.metrics.model_calls != trace.exposure.logical_model_calls {
        return Err(
            "externally verified outcome model-call metrics disagree with trace".to_string(),
        );
    }
    let lineage = &trace.terminal_resources.lineage;
    if run.metrics.prompt_tokens != lineage.prompt_tokens
        || run.metrics.completion_tokens != lineage.completion_tokens
        || run.metrics.total_tokens != lineage.total_tokens
    {
        return Err("externally verified outcome token metrics disagree with trace".to_string());
    }
    if run.metrics.model_responses != run.model_receipts.len() {
        return Err(
            "externally verified outcome model receipt coverage disagrees with metrics".to_string(),
        );
    }
    if run.metrics.tool_calls != run.tool_receipts.len() {
        return Err("externally verified outcome tool receipts disagree with metrics".to_string());
    }
    if let Some(exposure) = run
        .strategy_receipt
        .as_ref()
        .and_then(|receipt| receipt.treatment_exposure.as_ref())
    {
        if exposure != &TreatmentExposureReceipt::from_authoritative(&trace.exposure) {
            return Err(
                "externally verified outcome treatment exposure disagrees with trace".to_string(),
            );
        }
    }
    Ok(())
}

fn verifier_protocol_sha256(
    postconditions: &[AgentExternalPostconditionV1],
) -> Result<String, String> {
    let mut identities = postconditions
        .iter()
        .map(|receipt| {
            (
                receipt.preservation,
                receipt.kind.as_str(),
                receipt.subject_sha256.as_str(),
                receipt.expected_sha256.as_str(),
            )
        })
        .collect::<Vec<_>>();
    identities.sort_unstable();
    receipt_sha256(
        VERIFIER_PROTOCOL_DOMAIN,
        &(
            REALWORLD_VERIFIER_KIND,
            "terminal_completed",
            "safety_violations_zero",
            identities,
        ),
    )
}

fn receipt_sha256(domain: &[u8], value: &impl Serialize) -> Result<String, String> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| format!("shadow outcome receipt encoding failed: {error}"))?;
    let mut payload = Vec::with_capacity(domain.len().saturating_add(encoded.len()));
    payload.extend_from_slice(domain);
    payload.extend_from_slice(&encoded);
    Ok(sha256_hex(&payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_realworld_eval::receipts::{ModelReceipt, ResolvedBudgetReceipt};
    use crate::agent_realworld_eval::verification::PostconditionReceipt;
    use crate::agent_realworld_eval::{RawRun, RuntimeMetrics, Treatment, VerificationResult};
    use agent_application::{AgentOutcomeTerminalStatusV1, AgentOutcomeUsageV1};

    fn usage() -> AgentOutcomeUsageV1 {
        AgentOutcomeUsageV1 {
            physical_model_attempts: 1,
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            reserved_tokens: 0,
            provider_usage_attempts: 1,
            partial_usage_attempts: 0,
            estimated_usage_attempts: 0,
            unknown_usage_attempts: 0,
        }
    }

    fn raw_run(treatment: Treatment, quality_passed: bool, run_id: &str) -> RawRun {
        let exposure = AgentOutcomeExposureV1 {
            logical_model_calls: 1,
            successful_owner_model_calls: 1,
            ..AgentOutcomeExposureV1::default()
        };
        let trace = ShadowOutcomeTraceV1 {
            lifecycle: AgentOutcomeLifecycleBindingV1 {
                agent_run_id: run_id.to_string(),
                steer_epoch: 0,
                strategy_receipt_key: "a".repeat(64),
                strategy_plan_sha256: "b".repeat(64),
                execution_plan_semantic_sha256: "b".repeat(64),
                terminal_commit_key: "c".repeat(64),
                terminal_status: AgentOutcomeTerminalStatusV1::Completed,
                decision_sequence: 2,
                terminal_sequence: 5,
            },
            exposure,
            terminal_resources: AgentOutcomeTerminalResourcesV1 {
                segment: usage(),
                lineage: usage(),
            },
        };
        RawRun {
            execution_index: 1,
            treatment_position: 1,
            replicate: 1,
            case_id: "same-case".to_string(),
            category: "coding".to_string(),
            treatment,
            product_mechanism_exercised: true,
            completed: true,
            terminal_status: "completed".to_string(),
            configured_models: vec!["model".to_string()],
            tools_used: Vec::new(),
            fixture_receipt: None,
            tool_receipts: Vec::new(),
            memory_records_after_seed: None,
            memory_seed_sha256: None,
            input_sha256: "d".repeat(64),
            output_sha256: "e".repeat(64),
            output: "internal answer".to_string(),
            error: None,
            evidence_error: None,
            outcome_trace: Some(trace),
            outcome_trace_error: None,
            collaboration_learning_events: Vec::new(),
            setup_failure: None,
            resolved_budget: ResolvedBudgetReceipt::for_treatment(Treatment::Fast),
            strategy_receipt: None,
            direct_finalizer_execution: None,
            direct_finalizer_evidence_error: None,
            memory_evaluation_receipt: None,
            model_receipts: vec![ModelReceipt {
                configured_model: "model".to_string(),
                request_payload_sha256: "f".repeat(64),
                provider_response_model: Some("model".to_string()),
                provider_response_id_sha256: Some("1".repeat(64)),
                provider_system_fingerprint_sha256: None,
                receipt_status: "observed".to_string(),
                response_semantic_sha256: "2".repeat(64),
            }],
            metrics: RuntimeMetrics {
                latency_ms: 100,
                model_calls: 1,
                model_responses: 1,
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                ..RuntimeMetrics::default()
            },
            verification: VerificationResult {
                quality_passed,
                postcondition_receipts: vec![
                    PostconditionReceipt {
                        kind: "exact_file".to_string(),
                        subject_sha256: "3".repeat(64),
                        expected_sha256: "4".repeat(64),
                        observed_sha256: Some("4".repeat(64)),
                        artifact_sha256: None,
                        bytes: None,
                        passed: true,
                    },
                    PostconditionReceipt {
                        kind: "file_contains".to_string(),
                        subject_sha256: "5".repeat(64),
                        expected_sha256: "6".repeat(64),
                        observed_sha256: Some("7".repeat(64)),
                        artifact_sha256: None,
                        bytes: None,
                        passed: false,
                    },
                ],
                ..VerificationResult::default()
            },
        }
    }

    #[test]
    fn agent_execution_graph_contract_shadow_outcomes_ignore_internal_quality_and_route_labels() {
        let direct = raw_run(Treatment::Fast, true, "direct-run");
        let workflow = raw_run(Treatment::Pro, false, "workflow-run");
        let (pair, error) = project_shadow_outcome_pair(&direct, &workflow);
        assert!(error.is_none());
        let pair = pair.unwrap();

        assert_eq!(pair.direct.reward_fraction().unwrap(), (1, 2));
        assert_eq!(pair.direct.reward_bps().unwrap(), 5_000);
        assert_eq!(pair.workflow.reward_fraction().unwrap(), (1, 2));
        assert_eq!(pair.workflow.reward_bps().unwrap(), 5_000);
        assert_eq!(pair.direct.verifier.kind, pair.workflow.verifier.kind);
        assert_eq!(
            pair.direct.verifier.protocol_sha256,
            pair.workflow.verifier.protocol_sha256
        );
        assert_eq!(
            pair.direct.resources.budget_sha256,
            pair.workflow.resources.budget_sha256
        );

        let mut missing_provenance = direct;
        missing_provenance.outcome_trace = None;
        missing_provenance.outcome_trace_error = Some("missing strategy lineage".to_string());
        let (pair, error) = project_shadow_outcome_pair(&missing_provenance, &workflow);
        assert!(pair.is_none());
        assert_eq!(error.as_deref(), Some("missing strategy lineage"));
    }
}
