use agent_core::{Event, EventKind};
use orchestrator::{
    sha256_hex, LearningEvidenceV1, PromptLearningEligibilityReceiptV1,
    WorkflowExecutionCheckpoint, WorkflowPlanIr,
};
use std::collections::{BTreeMap, BTreeSet};

fn attestation_event_steer_epoch(event: &Event) -> &str {
    event
        .metadata
        .get("steer_epoch")
        .map(String::as_str)
        .unwrap_or("0")
}

pub(crate) fn prompt_auto_teacher_evaluator_receipt(
    run_events: &[&Event],
    terminal: &Event,
    collaboration_id: &str,
    stable_epoch: &str,
    quality_score_bps: u16,
) -> Option<orchestrator::AutoTeacherEvaluatorReceiptV1> {
    let quality_event = run_events.iter().rev().find(|event| {
        event.summary == "Collaboration quality gate evaluated"
            && attestation_event_steer_epoch(event) == stable_epoch
            && event.metadata.get("collaboration_id").map(String::as_str) == Some(collaboration_id)
            && event.sequence < terminal.sequence
            && event
                .metadata
                .contains_key(orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY)
    })?;
    let receipt = quality_event
        .metadata
        .get(orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY)
        .and_then(|encoded| {
            serde_json::from_str::<orchestrator::AutoTeacherEvaluatorReceiptV1>(encoded).ok()
        })?;
    receipt.validate().ok()?;
    let persisted_score_bps = quality_event
        .metadata
        .get("quality_score")
        .and_then(|score| score.parse::<f64>().ok())
        .filter(|score| score.is_finite() && (0.0..=1.0).contains(score))
        .map(|score| (score * 10_000.0).round() as u16)?;
    if receipt.quality_score_bps != quality_score_bps
        || persisted_score_bps != quality_score_bps
        || !receipt.passed
        || receipt.safety_violations > 0
        || quality_event
            .metadata
            .get("quality_pass")
            .map(String::as_str)
            != Some("true")
        || quality_event
            .metadata
            .get("safety_violations")
            .map(String::as_str)
            != Some("0")
    {
        return None;
    }
    let stage_event = run_events.iter().find(|event| {
        event.kind == EventKind::ModelRequestFinished
            && attestation_event_steer_epoch(event) == stable_epoch
            && event.metadata.get("collaboration_id").map(String::as_str) == Some(collaboration_id)
            && event.metadata.get("stage") == Some(&receipt.stage)
            && event.metadata.get("role").map(String::as_str) == Some("reviewer")
            && event.metadata.get("model") == Some(&receipt.evaluator.model)
            && event.metadata.get("status").map(String::as_str) == Some("completed")
            && event.metadata.get("usage_source").map(String::as_str) == Some("provider")
            && event.metadata.contains_key("output")
            && event.sequence < quality_event.sequence
            && event.metadata.get("request_id").is_some_and(|request_id| {
                sha256_hex(request_id.trim().as_bytes()) == receipt.request_id_sha256
            })
            && crate::prompt_learning_runtime::prompt_event_sha256(event)
                .is_ok_and(|digest| digest == receipt.stage_event_sha256)
    })?;
    let request_id = stage_event.metadata.get("request_id")?.trim();
    let request_events = run_events
        .iter()
        .copied()
        .filter(|event| {
            matches!(
                event.kind,
                EventKind::ModelRequestStarted | EventKind::ModelRequestFinished
            ) && attestation_event_steer_epoch(event) == stable_epoch
                && event.sequence < quality_event.sequence
                && event.metadata.get("request_id").map(|value| value.trim()) == Some(request_id)
        })
        .collect::<Vec<_>>();
    if request_events.len() != 2
        || request_events.iter().any(|event| {
            event.metadata.get("collaboration_id").map(String::as_str) != Some(collaboration_id)
                || event.metadata.get("stage") != Some(&receipt.stage)
                || event.metadata.get("role").map(String::as_str) != Some("reviewer")
                || event.metadata.get("model") != Some(&receipt.evaluator.model)
        })
    {
        return None;
    }
    let started_event = request_events
        .iter()
        .find(|event| event.kind == EventKind::ModelRequestStarted)?;
    if started_event.sequence >= stage_event.sequence
        || request_events
            .iter()
            .filter(|event| event.kind == EventKind::ModelRequestFinished)
            .count()
            != 1
    {
        return None;
    }
    Some(receipt)
}

fn prompt_runtime_event_is_in_epoch(event: &Event, terminal: &Event, stable_epoch: &str) -> bool {
    event.sequence < terminal.sequence && attestation_event_steer_epoch(event) == stable_epoch
}

fn prompt_runtime_event_model<'a>(
    event: &'a Event,
    started_models: &'a BTreeMap<String, String>,
) -> Option<&'a str> {
    event
        .metadata
        .get("model")
        .map(String::as_str)
        .or_else(|| {
            event
                .metadata
                .get("request_id")
                .and_then(|request_id| started_models.get(request_id))
                .map(String::as_str)
        })
        .map(str::trim)
        .filter(|model| !model.is_empty())
}

fn prompt_runtime_request_is_evaluator(
    event: &Event,
    receipt: &orchestrator::AutoTeacherEvaluatorReceiptV1,
) -> bool {
    event.metadata.get("request_id").is_some_and(|request_id| {
        sha256_hex(request_id.trim().as_bytes()) == receipt.request_id_sha256
    })
}

fn canonical_reviewed_artifact(output: &str, expected_sha256: &str) -> Option<String> {
    let artifact = orchestrator::canonical_auto_teacher_evaluated_artifact(output);
    orchestrator::auto_teacher_evaluated_artifact_sha256(&artifact)
        .is_ok_and(|digest| digest == expected_sha256)
        .then_some(artifact)
}

pub(crate) fn prompt_auto_teacher_runtime_attestation(
    run_events: &[&Event],
    terminal: &Event,
    collaboration_id: &str,
    stable_epoch: &str,
    checkpoint: &WorkflowExecutionCheckpoint,
    evaluator_receipt: &orchestrator::AutoTeacherEvaluatorReceiptV1,
) -> Option<(Vec<String>, String)> {
    evaluator_receipt.validate().ok()?;
    let started_models = run_events
        .iter()
        .filter(|event| event.kind == EventKind::ModelRequestStarted)
        .filter(|event| prompt_runtime_event_is_in_epoch(event, terminal, stable_epoch))
        .filter_map(|event| {
            Some((
                event.metadata.get("request_id")?.trim().to_string(),
                event.metadata.get("model")?.trim().to_string(),
            ))
        })
        .filter(|(request_id, model)| !request_id.is_empty() && !model.is_empty())
        .collect::<BTreeMap<_, _>>();
    let completed = run_events
        .iter()
        .copied()
        .filter(|event| event.kind == EventKind::ModelRequestFinished)
        .filter(|event| prompt_runtime_event_is_in_epoch(event, terminal, stable_epoch))
        .filter(|event| {
            event.metadata.get("status").map(String::as_str) == Some("completed")
                && event
                    .metadata
                    .get("output")
                    .is_some_and(|output| !output.trim().is_empty())
        })
        .collect::<Vec<_>>();

    let coordinator_was_observed = completed.iter().any(|event| {
        !prompt_runtime_request_is_evaluator(event, evaluator_receipt)
            && event.metadata.get("collaboration_id").map(String::as_str) == Some(collaboration_id)
            && event.metadata.get("role").map(String::as_str) == Some("planner")
            && event.metadata.get("stage").is_some_and(|stage| {
                stage == "conductor_plan" || stage.starts_with("conductor_repair")
            })
            && prompt_runtime_event_model(event, &started_models).is_some()
    });
    if !coordinator_was_observed {
        return None;
    }

    let mut participant_models = BTreeSet::new();
    for event in run_events
        .iter()
        .copied()
        .filter(|event| {
            matches!(
                event.kind,
                EventKind::ModelRequestStarted | EventKind::ModelRequestFinished
            )
        })
        .filter(|event| prompt_runtime_event_is_in_epoch(event, terminal, stable_epoch))
        .filter(|event| !prompt_runtime_request_is_evaluator(event, evaluator_receipt))
    {
        if let Some(model) = prompt_runtime_event_model(event, &started_models) {
            participant_models.insert(model.to_string());
        }
    }

    let mut evaluated_artifact = None;
    for (step_id, step) in &checkpoint.steps {
        if !matches!(
            step.status,
            orchestrator::WorkflowStepStatus::Completed
                | orchestrator::WorkflowStepStatus::Degraded
        ) {
            continue;
        }
        let output = step.output.as_deref()?.trim();
        let output_sha256 = orchestrator::auto_teacher_evaluated_artifact_sha256(output).ok()?;
        let author = completed.iter().find(|event| {
            !prompt_runtime_request_is_evaluator(event, evaluator_receipt)
                && event.metadata.get("collaboration_id").map(String::as_str)
                    == Some(collaboration_id)
                && event.metadata.get("workflow_step_id").map(String::as_str)
                    == Some(step_id.as_str())
                && prompt_runtime_event_model(event, &started_models) == Some(step.model.trim())
                && event.metadata.get("output").is_some_and(|event_output| {
                    orchestrator::auto_teacher_evaluated_artifact_sha256(event_output)
                        .is_ok_and(|digest| digest == output_sha256)
                })
        })?;
        participant_models.insert(prompt_runtime_event_model(author, &started_models)?.to_string());
        if output_sha256 == evaluator_receipt.evaluated_artifact_sha256 {
            evaluated_artifact =
                canonical_reviewed_artifact(output, &evaluator_receipt.evaluated_artifact_sha256);
        }
    }
    if evaluated_artifact.is_none() {
        evaluated_artifact = completed.iter().find_map(|event| {
            (!prompt_runtime_request_is_evaluator(event, evaluator_receipt)
                && event.metadata.get("collaboration_id").map(String::as_str)
                    == Some(collaboration_id))
            .then(|| event.metadata.get("output"))
            .flatten()
            .and_then(|output| {
                canonical_reviewed_artifact(output, &evaluator_receipt.evaluated_artifact_sha256)
            })
        });
    }
    if evaluated_artifact.is_none() || participant_models.is_empty() {
        return None;
    }

    // The provider receipt proves a separate evaluator request. Model overlap remains explicit in
    // `participant_models` and the evaluator identity instead of being mislabeled as diversity.
    Some((
        participant_models.into_iter().collect(),
        evaluated_artifact?,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prompt_auto_teacher_source_context(
    project_id: &str,
    stable_epoch: &str,
    run_events: &[&Event],
    profile_event: &Event,
    plan: &WorkflowPlanIr,
    checkpoint: &WorkflowExecutionCheckpoint,
    learning_evidence: &LearningEvidenceV1,
    learning_receipt: &PromptLearningEligibilityReceiptV1,
    participant_models: &[String],
    evaluator_receipt: &orchestrator::AutoTeacherEvaluatorReceiptV1,
    profile_sha256: &str,
) -> Option<orchestrator::AutoTeacherSourceContextV1> {
    let provider_sha256 = evaluator_receipt.evaluator.provider_digest().ok()?;
    let mut participant_identities = participant_models
        .iter()
        .map(|model| evaluator_receipt.evaluator.with_model(model).ok())
        .collect::<Option<Vec<_>>>()?;
    participant_identities.sort_by(|left, right| left.identity_sha256.cmp(&right.identity_sha256));
    participant_identities.dedup_by(|left, right| left.identity_sha256 == right.identity_sha256);
    if participant_identities.is_empty() {
        return None;
    }
    let model_pool_sha256 = serde_json::to_vec(
        &participant_identities
            .iter()
            .map(|identity| identity.identity_sha256.as_str())
            .collect::<Vec<_>>(),
    )
    .ok()
    .map(|encoded| sha256_hex(&encoded))?;
    let policy = profile_event.metadata.get("collaboration_policy")?.trim();
    if policy.is_empty() {
        return None;
    }
    let policy_sha256 =
        sha256_hex(format!("effort=auto\npolicy={policy}\nprofile={profile_sha256}").as_bytes());
    let budget_fingerprint = learning_evidence.budget_fingerprint.as_deref()?;
    let budget_sha256 = serde_json::to_vec(&(budget_fingerprint, &plan.budget))
        .ok()
        .map(|encoded| sha256_hex(&encoded))?;
    let checkpoint_sha256 = serde_json::to_vec(checkpoint)
        .ok()
        .map(|encoded| sha256_hex(&encoded))?;
    let learning_receipt_sha256 = serde_json::to_vec(learning_receipt)
        .ok()
        .map(|encoded| sha256_hex(&encoded))?;
    let project_root = profile_event.metadata.get("project_root")?.trim();
    if project_root.is_empty() {
        return None;
    }
    let workspace_evidence = run_events
        .iter()
        .filter(|event| {
            attestation_event_steer_epoch(event) == stable_epoch
                && matches!(
                    &event.kind,
                    EventKind::ToolCallProposed
                        | EventKind::ToolCallStarted
                        | EventKind::ToolCallFinished
                        | EventKind::RetrievalPerformed
                )
        })
        .map(|event| crate::prompt_learning_runtime::prompt_event_sha256(event).ok())
        .collect::<Option<Vec<_>>>()?;
    let workspace_revision_sha256 = serde_json::to_vec(&(
        sha256_hex(project_id.trim().as_bytes()),
        sha256_hex(project_root.as_bytes()),
        workspace_evidence,
        &checkpoint_sha256,
    ))
    .ok()
    .map(|encoded| sha256_hex(&encoded))?;
    let context = orchestrator::AutoTeacherSourceContextV1 {
        schema: orchestrator::AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1.to_string(),
        provider_sha256,
        model_pool_sha256,
        system_prompt_sha256: evaluator_receipt.system_prompt_sha256.clone(),
        policy_sha256,
        budget_sha256,
        tool_contract_sha256: evaluator_receipt.tool_contract_sha256.clone(),
        source_revision_sha256: evaluator_receipt.source_revision_sha256.clone(),
        workspace_revision_sha256,
        evaluator_identity_sha256: evaluator_receipt.evaluator.identity_sha256.clone(),
        evaluator_receipt_sha256: evaluator_receipt.digest().ok()?,
        checkpoint_sha256,
        learning_receipt_sha256,
    };
    context.validate().ok()?;
    Some(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_teacher_output_reconstructs_the_reviewed_canonical_artifact() {
        let output = "x"
            .repeat(orchestrator::AUTO_TEACHER_EVALUATED_ARTIFACT_MAX_CHARS.saturating_add(8_192));
        let digest = orchestrator::auto_teacher_evaluated_artifact_sha256(&output).unwrap();
        let reconstructed = canonical_reviewed_artifact(&output, &digest).unwrap();

        assert_eq!(
            reconstructed,
            orchestrator::canonical_auto_teacher_evaluated_artifact(&output)
        );
        assert!(reconstructed.ends_with("\n[truncated]"));
    }
}
