use super::workflow_gepa_campaign_contract::{CampaignSplit, ProductRunReceipt};
use super::workflow_gepa_campaign_contract::{CAMPAIGN_SUITE_ID, CAMPAIGN_VERSION};
use super::workflow_gepa_campaign_execution::MatchedRoutePairRun;
use super::{RawRun, RealworldCase, RealworldSuite};
use orchestrator::{
    sha256_hex, ActionableSideInformation, AgentEvaluationCheck, AgentEvaluationEvidenceSource,
    AgentEvaluationReflectionPacket, AgentEvaluationToolTrace, AgentEvaluationTraceStep,
    AgentEvaluationVerifierOutcome, ConductorPromptGenome,
};
use std::collections::BTreeMap;

const MAX_REFLECTION_OUTPUT_CHARS: usize = 8_000;

pub(super) fn route_treatment_reflection_packets(
    suite: &RealworldSuite,
    seed_profile: &ConductorPromptGenome,
    pairs: &[(usize, MatchedRoutePairRun)],
) -> Result<Vec<AgentEvaluationReflectionPacket>, String> {
    let mut packets = Vec::with_capacity(pairs.len().saturating_mul(2));
    for (seed, pair) in pairs {
        let case = suite
            .cases
            .iter()
            .find(|case| case.id == pair.direct.case_id)
            .ok_or_else(|| {
                format!(
                    "training pair {} is absent from the suite",
                    pair.direct.case_id
                )
            })?;
        if CampaignSplit::parse(case)? != CampaignSplit::Train
            || pair.direct.case_id != pair.workflow.case_id
        {
            return Err(format!(
                "matched route pair {} is not a valid training pair",
                case.id
            ));
        }
        let run_id = format!("route-treatment-{}-{seed}", case.id);
        packets.push(reflection_packet(
            case,
            seed_profile,
            *seed as u64,
            "forced_direct",
            &run_id,
            &pair.direct,
        )?);
        packets.push(reflection_packet(
            case,
            seed_profile,
            *seed as u64,
            "forced_workflow",
            &run_id,
            &pair.workflow,
        )?);
    }
    if packets.len() != 4
        || packets
            .iter()
            .map(|packet| packet.category.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != 2
    {
        return Err(
            "route learning requires matched Direct and Workflow runs for one coding and one research case"
                .to_string(),
        );
    }
    Ok(packets)
}

fn reflection_packet(
    case: &RealworldCase,
    seed_profile: &ConductorPromptGenome,
    seed: u64,
    candidate_id: &str,
    run_id: &str,
    run: &RawRun,
) -> Result<AgentEvaluationReflectionPacket, String> {
    let receipt = ProductRunReceipt::from_run(run, CampaignSplit::Train, 1)?;
    let mut behavior = run
        .verification
        .postcondition_receipts
        .iter()
        .filter(|postcondition| postcondition.kind != "immutable_fixture")
        .map(|postcondition| AgentEvaluationCheck {
            id: format!(
                "{}-{}",
                postcondition.kind,
                &postcondition.subject_sha256[..12]
            ),
            passed: postcondition.passed,
            detail: format!(
                "External {} behavior postcondition {}",
                postcondition.kind,
                if postcondition.passed {
                    "passed"
                } else {
                    "failed"
                }
            ),
        })
        .collect::<Vec<_>>();
    if behavior.is_empty() {
        return Err(format!(
            "training case {} has no behavior evidence",
            case.id
        ));
    }
    behavior.push(AgentEvaluationCheck {
        id: "agent_terminal_completed".to_string(),
        passed: run.completed,
        detail: format!(
            "Product Agent terminal status {}",
            if run.completed { "completed" } else { "failed" }
        ),
    });
    behavior.push(AgentEvaluationCheck {
        id: "safety_violations_zero".to_string(),
        passed: run.verification.safety_violations == 0,
        detail: format!(
            "Product Agent safety violations {}",
            run.verification.safety_violations
        ),
    });
    let (verifier_passed, verifier_score) = external_verifier_outcome(&behavior);
    let failed_constraints = behavior
        .iter()
        .filter(|check| !check.passed)
        .map(|check| check.detail.clone())
        .collect::<Vec<_>>();
    let errors = run
        .error
        .iter()
        .map(|error| truncate(error, 500))
        .collect::<Vec<_>>();
    let suggested_changes = if verifier_passed {
        vec![
            "Preserve the verified behavior while reducing unnecessary model work and latency."
                .to_string(),
        ]
    } else {
        vec![
            "Read the complete public contract before planning and verify every stated behavior before final synthesis."
                .to_string(),
            "Use tool evidence to repair failed behavior rather than treating an internally completed run as success."
                .to_string(),
        ]
    };
    let model_fingerprints = run
        .model_receipts
        .iter()
        .enumerate()
        .map(|(index, model)| {
            (
                format!("model-{index}"),
                sha256_hex(model.configured_model.as_bytes()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let tool_calls = run
        .tool_receipts
        .iter()
        .map(|tool| AgentEvaluationToolTrace {
            tool: tool.tool.clone(),
            request: tool
                .input_fingerprint
                .as_ref()
                .map(|fingerprint| format!("input_sha256={fingerprint}"))
                .unwrap_or_else(|| "input_sha256=unavailable".to_string()),
            response: format!("status={:?}", tool.status),
            error: None,
        })
        .collect::<Vec<_>>();
    let public_input = public_case_input(case);
    Ok(AgentEvaluationReflectionPacket {
        suite_id: CAMPAIGN_SUITE_ID.to_string(),
        suite_version: CAMPAIGN_VERSION,
        case_id: case.id.clone(),
        category: case.category.clone(),
        run_id: run_id.to_string(),
        seed,
        candidate_id: candidate_id.to_string(),
        candidate_fingerprint: orchestrator::prompt_genome_sha256(seed_profile)?,
        model_fingerprints,
        input: public_input,
        steps: vec![AgentEvaluationTraceStep {
            step_id: "full-product-agent-run".to_string(),
            role: "product_agent".to_string(),
            model: "provider-backed-model-set".to_string(),
            prompt: case.objective.clone(),
            output: truncate(&run.output, MAX_REFLECTION_OUTPUT_CHARS),
            tool_calls,
            errors: errors.clone(),
            latency_ms: run.metrics.latency_ms,
            total_tokens: run.metrics.total_tokens,
        }],
        final_output: truncate(&run.output, MAX_REFLECTION_OUTPUT_CHARS),
        verifier: AgentEvaluationVerifierOutcome {
            source: AgentEvaluationEvidenceSource::Deterministic,
            passed: verifier_passed,
            score: verifier_score,
            checks: behavior,
        },
        actionable_feedback: ActionableSideInformation {
            summary: format!(
                "Full product run: completed={}; behavior={}/{}; latency_ms={}; total_tokens={}; workflow_profile_exercised={}; external verifier is authoritative over the internal terminal status.",
                receipt.completed,
                receipt.behavior_checks_passed,
                receipt.behavior_checks_total,
                receipt.latency_ms,
                receipt.total_tokens,
                receipt.workflow_profile_exercised,
            ),
            passed_constraints: if verifier_passed {
                vec!["All external behavior postconditions passed.".to_string()]
            } else {
                Vec::new()
            },
            failed_constraints,
            errors,
            suggested_changes,
        },
    })
}

fn public_case_input(case: &RealworldCase) -> String {
    let fixtures = case
        .files
        .iter()
        .map(|fixture| format!("[{}]\n{}", fixture.path, fixture.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    truncate(
        &format!(
            "Objective:\n{}\n\nPublic fixtures:\n{fixtures}",
            case.objective
        ),
        MAX_REFLECTION_OUTPUT_CHARS,
    )
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}\n[TRUNCATED]")
    } else {
        truncated
    }
}

fn external_verifier_outcome(checks: &[AgentEvaluationCheck]) -> (bool, f64) {
    let passed = !checks.is_empty() && checks.iter().all(|check| check.passed);
    let score =
        checks.iter().filter(|check| check.passed).count() as f64 / checks.len().max(1) as f64;
    (passed, score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_failure_overrides_internal_completion_for_reflection() {
        let checks = vec![
            AgentEvaluationCheck {
                id: "public-check".to_string(),
                passed: true,
                detail: "Public check passed".to_string(),
            },
            AgentEvaluationCheck {
                id: "external-behavior".to_string(),
                passed: false,
                detail: "External behavior postcondition failed".to_string(),
            },
        ];
        let (passed, score) = external_verifier_outcome(&checks);
        assert!(!passed);
        assert_eq!(score, 0.5);

        let (passed, score) = external_verifier_outcome(&checks[..1]);
        assert!(passed);
        assert_eq!(score, 1.0);
    }
}
