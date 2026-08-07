use crate::{
    app_state::AgentRecoveryEnvelope, runtime_constants::AGENT_RECOVERY_SCHEMA,
    view_models::PromptFailureCurriculumRecord,
};
use agent_application::{AgentRecoveryReason, AgentRecoveryState, AgentRunEvent, AgentRunStatus};
use agent_core::Event;
use agent_runtime::{
    AgentActionDenialKind, ContractEvidenceKind, OutcomeFailureClass, OutcomeLedgerPhase,
    OutcomeLedgerShadow, OutcomeSatisfaction,
};
use orchestrator::{
    sha256_hex, PromptFailureCurriculumInput, PromptFailureCurriculumKind,
    PromptFailureCurriculumReceiptV1, PromptFailureDenialKind, WorkflowPlanIr,
};
use std::collections::BTreeMap;

pub(crate) fn terminal_outcome_ledger_has_blocking_denial(terminal: &Event) -> bool {
    let Some(ledger) = OutcomeLedgerShadow::from_terminal_metadata(
        &terminal.metadata,
        OutcomeLedgerPhase::Completed,
    ) else {
        return false;
    };
    ledger.obligations.iter().any(|obligation| {
        obligation.satisfaction == OutcomeSatisfaction::Blocked
            && obligation.blocker.is_some()
            && obligation.evidence_sequence.is_some_and(|sequence| {
                ledger.evidence.iter().any(|evidence| {
                    evidence.sequence == sequence && evidence.kind == ContractEvidenceKind::Denial
                })
            })
    })
}

pub(crate) fn prompt_failure_curriculum_records_from_events(
    events: &[Event],
) -> Vec<(u64, PromptFailureCurriculumRecord)> {
    let mut runs = BTreeMap::<String, Vec<&Event>>::new();
    for event in events {
        if let Some(run_id) = event.metadata.get("agent_run_id") {
            runs.entry(run_id.clone()).or_default().push(event);
        }
    }
    runs.into_iter()
        .flat_map(|(run_id, mut run_events)| {
            run_events.sort_by_key(|event| event.sequence);
            prompt_failure_curriculum_for_run(&run_id, &run_events)
        })
        .collect()
}

fn prompt_failure_curriculum_for_run(
    run_id: &str,
    run_events: &[&Event],
) -> Vec<(u64, PromptFailureCurriculumRecord)> {
    let Some(outcome) = run_events.iter().rev().find(|event| {
        matches!(
            AgentRunEvent::from_event(event),
            Some(
                AgentRunEvent::Paused
                    | AgentRunEvent::Completed
                    | AgentRunEvent::Failed
                    | AgentRunEvent::Cancelled
            )
        )
    }) else {
        return Vec::new();
    };
    let Some(status) = AgentRunEvent::from_event(outcome).map(AgentRunEvent::status) else {
        return Vec::new();
    };
    if status == AgentRunStatus::Cancelled {
        return Vec::new();
    }
    let Some(steer_epoch) = event_steer_epoch(outcome) else {
        return Vec::new();
    };
    let Some(profile_event) = run_events.iter().rev().find(|event| {
        event.sequence < outcome.sequence
            && event_steer_epoch(event) == Some(steer_epoch)
            && matches!(
                event.summary.as_str(),
                "Conductor prompt profile selected" | "Collaboration workflow planned"
            )
    }) else {
        return Vec::new();
    };
    let plan = profile_event
        .metadata
        .get("workflow_ir")
        .and_then(|encoded| serde_json::from_str::<WorkflowPlanIr>(encoded).ok());
    let Some(profile_id) = profile_event
        .metadata
        .get("prompt_profile")
        .cloned()
        .or_else(|| plan.as_ref().map(|plan| plan.prompt_profile.clone()))
    else {
        return Vec::new();
    };
    let Some(effort) = profile_event
        .metadata
        .get("prompt_effort")
        .cloned()
        .or_else(|| plan.as_ref().map(|plan| plan.effort.clone()))
    else {
        return Vec::new();
    };
    if effort != "pro" {
        return Vec::new();
    }
    let scope = profile_event
        .metadata
        .get("project_id")
        .cloned()
        .unwrap_or_else(|| "global".to_string());
    let policy = profile_event
        .metadata
        .get("collaboration_policy")
        .or_else(|| profile_event.metadata.get("requested_policy"))
        .cloned()
        .unwrap_or_else(|| effort.clone());
    let task_class = profile_event
        .metadata
        .get("task_class")
        .cloned()
        .unwrap_or_else(|| "general".to_string());
    let context = FailureCurriculumContext {
        scope,
        effort,
        run_id,
        profile_id: &profile_id,
        policy: &policy,
        task_class: &task_class,
        steer_epoch,
        outcome,
    };
    match status {
        AgentRunStatus::Completed => completed_denial_curricula(context),
        AgentRunStatus::Failed => failed_run_curriculum(context).into_iter().collect(),
        AgentRunStatus::Paused => paused_run_curriculum(context).into_iter().collect(),
        _ => Vec::new(),
    }
}

struct FailureCurriculumContext<'a> {
    scope: String,
    effort: String,
    run_id: &'a str,
    profile_id: &'a str,
    policy: &'a str,
    task_class: &'a str,
    steer_epoch: u64,
    outcome: &'a Event,
}

fn completed_denial_curricula(
    context: FailureCurriculumContext<'_>,
) -> Vec<(u64, PromptFailureCurriculumRecord)> {
    let Some(ledger) = OutcomeLedgerShadow::from_terminal_metadata(
        &context.outcome.metadata,
        OutcomeLedgerPhase::Completed,
    )
    .filter(|ledger| ledger.steer_epoch == context.steer_epoch) else {
        return Vec::new();
    };
    let Some(source_digest) = context
        .outcome
        .metadata
        .get(agent_runtime::OUTCOME_LEDGER_DIGEST_METADATA_KEY)
    else {
        return Vec::new();
    };
    ledger
        .obligations
        .iter()
        .filter(|obligation| obligation.satisfaction == OutcomeSatisfaction::Blocked)
        .filter_map(|obligation| {
            let blocker = obligation.blocker.as_ref()?;
            let sequence = obligation.evidence_sequence?;
            let evidence = ledger.evidence.iter().find(|evidence| {
                evidence.sequence == sequence && evidence.kind == ContractEvidenceKind::Denial
            })?;
            build_record(
                &context,
                PromptFailureCurriculumKind::Denial,
                ledger.steer_epoch,
                source_digest,
                &blocker.code,
                Some(prompt_denial_kind(blocker.kind)),
                Some(&evidence.source),
                Some(&evidence.input_fingerprint),
            )
        })
        .collect()
}

fn failed_run_curriculum(
    context: FailureCurriculumContext<'_>,
) -> Option<(u64, PromptFailureCurriculumRecord)> {
    let ledger = OutcomeLedgerShadow::from_terminal_metadata(
        &context.outcome.metadata,
        OutcomeLedgerPhase::Failed,
    )?;
    if ledger.steer_epoch != context.steer_epoch {
        return None;
    }
    let failure = ledger.failure.as_ref()?;
    let kind = match (failure.class, failure.code.as_str()) {
        (OutcomeFailureClass::Contract, "no_progress" | "repeated_action") => {
            PromptFailureCurriculumKind::NoProgress
        }
        (OutcomeFailureClass::Budget, "deadline_exceeded") => PromptFailureCurriculumKind::Timeout,
        _ => return None,
    };
    let source_digest = context
        .outcome
        .metadata
        .get(agent_runtime::OUTCOME_LEDGER_DIGEST_METADATA_KEY)?;
    build_record(
        &context,
        kind,
        ledger.steer_epoch,
        source_digest,
        &failure.code,
        None,
        None,
        None,
    )
}

fn paused_run_curriculum(
    context: FailureCurriculumContext<'_>,
) -> Option<(u64, PromptFailureCurriculumRecord)> {
    let encoded = context.outcome.metadata.get("recovery_envelope")?;
    let envelope = serde_json::from_str::<AgentRecoveryEnvelope>(encoded).ok()?;
    if envelope.schema != AGENT_RECOVERY_SCHEMA
        || envelope.identity.validate().is_err()
        || envelope
            .task_state
            .as_ref()
            .is_some_and(|snapshot| snapshot.validate_checkpoint().is_err())
    {
        return None;
    }
    if envelope.state != AgentRecoveryState::Paused
        || envelope.identity.source_run_id != context.run_id
        || envelope.identity.project_id.as_deref().unwrap_or("global") != context.scope
        || context
            .outcome
            .metadata
            .get("recovery_reason")
            .map(String::as_str)
            != Some(envelope.reason.label())
        || context
            .outcome
            .metadata
            .get("stop_reason")
            .map(String::as_str)
            != Some(envelope.reason.label())
    {
        return None;
    }
    let kind = match envelope.reason {
        AgentRecoveryReason::DeadlineExceeded => PromptFailureCurriculumKind::Timeout,
        AgentRecoveryReason::NoProgress | AgentRecoveryReason::RepeatedAction => {
            PromptFailureCurriculumKind::NoProgress
        }
        _ => return None,
    };
    let source_digest = sha256_hex(encoded.as_bytes());
    build_record(
        &context,
        kind,
        context.steer_epoch,
        &source_digest,
        envelope.reason.label(),
        None,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_record(
    context: &FailureCurriculumContext<'_>,
    kind: PromptFailureCurriculumKind,
    contract_epoch: u64,
    source_digest: &str,
    failure_code: &str,
    denial_kind: Option<PromptFailureDenialKind>,
    tool_name: Option<&str>,
    input_fingerprint: Option<&str>,
) -> Option<(u64, PromptFailureCurriculumRecord)> {
    let receipt = PromptFailureCurriculumReceiptV1::new(PromptFailureCurriculumInput {
        kind,
        project_id: &context.scope,
        run_id: context.run_id,
        profile_id: context.profile_id,
        policy: context.policy,
        task_class: context.task_class,
        steer_epoch: context.steer_epoch,
        contract_epoch,
        source_event_sequence: context.outcome.sequence,
        source_evidence_sha256: source_digest,
        failure_code,
        denial_kind,
        tool_name,
        input_fingerprint,
    })
    .ok()?;
    Some((
        context.outcome.sequence,
        PromptFailureCurriculumRecord {
            scope: context.scope.clone(),
            effort: context.effort.clone(),
            receipt,
        },
    ))
}

fn prompt_denial_kind(kind: AgentActionDenialKind) -> PromptFailureDenialKind {
    match kind {
        AgentActionDenialKind::UserPermission => PromptFailureDenialKind::UserPermission,
        AgentActionDenialKind::RuntimePolicy => PromptFailureDenialKind::RuntimePolicy,
        AgentActionDenialKind::CapabilityUnavailable => {
            PromptFailureDenialKind::CapabilityUnavailable
        }
        AgentActionDenialKind::RepeatedAction => PromptFailureDenialKind::RepeatedAction,
    }
}

fn event_steer_epoch(event: &Event) -> Option<u64> {
    event
        .metadata
        .get("steer_epoch")
        .and_then(|epoch| epoch.parse::<u64>().ok())
}
