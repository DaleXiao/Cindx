use crate::prompt_evolution_read_model::scoped_prompt_evaluation_id;
use agent_application::{AgentRunEvent, AgentRunStatus};
use agent_core::{Event, EventKind};
use orchestrator::{
    IndependentQualitySource, LearningAttribution, LearningDisposition, LearningEvidenceV1,
    LearningUsageCompleteness, PromptEvaluationMode, PromptEvaluationSplit,
    PromptEvolutionObservation, PromptStepCredit, WorkflowPlanIr,
};
use std::collections::BTreeMap;

fn is_agent_run_terminal(event: &Event) -> bool {
    AgentRunEvent::from_event(event).is_some_and(|event| event.status().is_terminal())
}

fn is_agent_run_completed(event: &Event) -> bool {
    AgentRunEvent::from_event(event).map(AgentRunEvent::status) == Some(AgentRunStatus::Completed)
}

fn trusted_run_lineage_total_tokens(
    terminal: &Event,
    evidence: &LearningEvidenceV1,
) -> Option<u64> {
    let usage =
        crate::learning_evidence_runtime::learning_lineage_usage_from_metadata(&terminal.metadata)?;
    (usage.completeness == evidence.usage_completeness
        && usage.completeness != LearningUsageCompleteness::Missing)
        .then_some(usage.total_tokens)
}

pub(crate) fn prompt_live_canary_outcome_records_from_events(
    events: &[Event],
) -> Vec<(u64, String, PromptEvolutionObservation)> {
    let mut workflows = BTreeMap::<String, Vec<&Event>>::new();
    let mut agent_runs = BTreeMap::<String, Vec<&Event>>::new();
    for event in events {
        if let Some(workflow_id) = event.metadata.get("collaboration_id") {
            workflows
                .entry(workflow_id.clone())
                .or_default()
                .push(event);
        }
        if let Some(run_id) = event.metadata.get("agent_run_id") {
            agent_runs.entry(run_id.clone()).or_default().push(event);
        }
    }
    workflows
        .into_iter()
        .filter_map(|(workflow_id, mut workflow_events)| {
            workflow_events.sort_by_key(|event| event.sequence);
            let profile_event = workflow_events
                .iter()
                .rev()
                .find(|event| event.summary == "Conductor prompt profile selected")
                .copied()
                .or_else(|| {
                    workflow_events
                        .iter()
                        .rev()
                        .find(|event| event.summary == "Collaboration workflow planned")
                        .copied()
                })?;
            let steer_epoch = profile_event
                .metadata
                .get("steer_epoch")
                .and_then(|value| value.parse::<u64>().ok())?;
            let stable_workflow_events = workflow_events
                .iter()
                .copied()
                .filter(|event| {
                    event
                        .metadata
                        .get("steer_epoch")
                        .and_then(|value| value.parse::<u64>().ok())
                        == Some(steer_epoch)
                })
                .collect::<Vec<_>>();
            let plan = stable_workflow_events
                .iter()
                .find(|event| event.summary == "Collaboration workflow planned")
                .and_then(|event| event.metadata.get("workflow_ir"))
                .and_then(|value| serde_json::from_str::<WorkflowPlanIr>(value).ok());
            let profile_id = profile_event
                .metadata
                .get("prompt_profile")
                .cloned()
                .or_else(|| plan.as_ref().map(|plan| plan.prompt_profile.clone()))?;
            let effort = profile_event
                .metadata
                .get("prompt_effort")
                .cloned()
                .or_else(|| plan.as_ref().map(|plan| plan.effort.clone()))?;
            let evidence_scope = profile_event
                .metadata
                .get("project_id")
                .map(String::as_str)
                .unwrap_or("global");
            let bounded_profile = profile_event
                .metadata
                .get("collaboration_profile")
                .map(String::as_str)
                == Some("bounded");
            let workflow_terminal = stable_workflow_events
                .iter()
                .rev()
                .find(|event| {
                    matches!(
                        event.summary.as_str(),
                        "Collaboration workflow completed" | "Collaboration workflow failed"
                    ) && event.sequence > profile_event.sequence
                })
                .copied()?;
            let workflow_completed =
                workflow_terminal.summary == "Collaboration workflow completed";
            let run_events = profile_event
                .metadata
                .get("agent_run_id")
                .and_then(|run_id| agent_runs.get(run_id));
            let terminal = if let Some(run_events) = run_events {
                run_events
                    .iter()
                    .rev()
                    .find(|event| {
                        is_agent_run_terminal(event)
                            && event.metadata.get("collaboration_id").map(String::as_str)
                                == Some(workflow_id.as_str())
                            && event.sequence > workflow_terminal.sequence
                            && event
                                .metadata
                                .get("steer_epoch")
                                .and_then(|value| value.parse::<u64>().ok())
                                == Some(steer_epoch)
                    })
                    .copied()
                    .or_else(|| (!workflow_completed).then_some(workflow_terminal))
            } else {
                Some(workflow_terminal)
            }?;
            let completed = if run_events.is_some() {
                workflow_completed && is_agent_run_completed(terminal)
            } else {
                workflow_completed
            };
            let learning_evidence = completed
                .then(|| LearningEvidenceV1::from_metadata(&terminal.metadata))
                .flatten()
                .filter(|evidence| {
                    evidence.is_learnable()
                        && evidence.steer_epoch == Some(steer_epoch)
                        && evidence.attribution == LearningAttribution::Workflow
                        && evidence.independent_quality_source.is_some()
                        && evidence.usage_completeness != LearningUsageCompleteness::Missing
                })
                .filter(|evidence| trusted_run_lineage_total_tokens(terminal, evidence).is_some());
            let evidence_succeeded = learning_evidence
                .as_ref()
                .is_some_and(|evidence| evidence.disposition == LearningDisposition::Positive);
            let measured_quality = learning_evidence
                .as_ref()
                .and_then(LearningEvidenceV1::quality_score)
                .map(f64::from)
                .unwrap_or(if evidence_succeeded { 1.0 } else { 0.0 });
            let quality_event = stable_workflow_events
                .iter()
                .rev()
                .find(|event| event.summary == "Collaboration quality gate evaluated");
            let measured_safety_violations = quality_event
                .and_then(|event| event.metadata.get("safety_violations"))
                .and_then(|count| count.parse::<u64>().ok())
                .unwrap_or_default();
            let evaluation_events = run_events
                .map(|events| events.as_slice())
                .unwrap_or(workflow_events.as_slice())
                .iter()
                .copied()
                .filter(|event| {
                    event.metadata.get("collaboration_id").map(String::as_str)
                        == Some(workflow_id.as_str())
                        && event
                            .metadata
                            .get("steer_epoch")
                            .and_then(|value| value.parse::<u64>().ok())
                            == Some(steer_epoch)
                })
                .collect::<Vec<_>>();
            let permission_denials = evaluation_events
                .iter()
                .filter(|event| {
                    event.kind == EventKind::PermissionResolved
                        && event.metadata.get("decision").is_some_and(|decision| {
                            !matches!(decision.as_str(), "allow_once" | "allow_for_session")
                        })
                })
                .count() as u64;
            let succeeded = evidence_succeeded && permission_denials == 0;
            let total_tokens = learning_evidence
                .as_ref()
                .and_then(|evidence| trusted_run_lineage_total_tokens(terminal, evidence))
                .or_else(|| {
                    crate::learning_evidence_runtime::learning_lineage_usage_from_metadata(
                        &terminal.metadata,
                    )
                    .filter(|usage| usage.completeness != LearningUsageCompleteness::Missing)
                    .map(|usage| usage.total_tokens)
                })
                .unwrap_or_default();
            let step_credits = stable_workflow_events
                .iter()
                .rev()
                .find_map(|event| event.metadata.get("step_credits"))
                .and_then(|encoded| serde_json::from_str::<Vec<PromptStepCredit>>(encoded).ok())
                .unwrap_or_default();
            let relative_reward = if succeeded
                && learning_evidence.as_ref().is_some_and(|evidence| {
                    evidence.independent_quality_source
                        == Some(IndependentQualitySource::AnytimeSelector)
                }) {
                workflow_terminal
                    .metadata
                    .get("anytime_team_uplift_bps")
                    .and_then(|uplift| uplift.parse::<i16>().ok())
                    .filter(|uplift| (-10_000..=10_000).contains(uplift))
                    .map(|uplift| f64::from(uplift) / 10_000.0)
            } else {
                None
            };
            Some((
                terminal.sequence,
                effort,
                PromptEvolutionObservation {
                    profile_id,
                    evaluation_id: scoped_prompt_evaluation_id(evidence_scope, &workflow_id),
                    case_id: workflow_id,
                    opponent_profile_id: None,
                    task_class: profile_event
                        .metadata
                        .get("task_class")
                        .cloned()
                        .unwrap_or_else(|| "general".to_string()),
                    split: PromptEvaluationSplit::Train,
                    mode: PromptEvaluationMode::Live,
                    format_valid: completed
                        && permission_denials == 0
                        && learning_evidence.is_some()
                        && (plan.is_some() || bounded_profile),
                    succeeded,
                    quality_score: measured_quality,
                    latency_ms: terminal
                        .timestamp_ms
                        .saturating_sub(profile_event.timestamp_ms),
                    total_tokens,
                    estimated_cost_microusd: 0,
                    safety_violations: measured_safety_violations
                        .saturating_add(permission_denials),
                    relative_reward,
                    step_credits,
                    reflection_packet: None,
                    provenance: Default::default(),
                },
            ))
        })
        .collect()
}
