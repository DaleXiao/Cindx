use super::*;
use orchestrator::{
    LearningAttribution, LearningEvidenceV1, LearningTermination, LearningUsageCompleteness,
};

pub(crate) const LEARNING_BUDGET_KEYS: [&str; 12] = [
    "run_budget_ms",
    "run_model_call_budget",
    "run_initial_model_calls",
    "run_tool_call_budget",
    "run_initial_tool_calls",
    "run_model_timeout_ms",
    "run_tool_timeout_ms",
    "run_no_progress_ms",
    "run_total_token_budget",
    "run_physical_model_attempt_budget",
    "run_terminal_token_reserve",
    "run_terminal_physical_model_attempt_reserve",
];

pub(crate) fn learning_budget_fingerprint(metadata: &Metadata) -> Option<String> {
    let canonical = LEARNING_BUDGET_KEYS
        .into_iter()
        .map(|key| {
            metadata
                .get(key)?
                .parse::<u64>()
                .ok()
                .map(|value| format!("{key}={value}"))
        })
        .collect::<Option<Vec<_>>>()?
        .join("\n");
    Some(sha256_hex(canonical.as_bytes()))
}

#[cfg_attr(not(test), allow(dead_code))]
fn learning_usage_completeness(events: &[&Event]) -> LearningUsageCompleteness {
    let model_events = events
        .iter()
        .filter(|event| event.kind == EventKind::ModelRequestFinished)
        .collect::<Vec<_>>();
    if model_events.is_empty() {
        return LearningUsageCompleteness::Missing;
    }
    let mut complete = true;
    for event in model_events {
        if event
            .metadata
            .get("total_tokens")
            .and_then(|value| value.parse::<u64>().ok())
            .is_none()
        {
            return LearningUsageCompleteness::Missing;
        }
        match event.metadata.get("usage_source").map(String::as_str) {
            Some("provider") => {}
            Some("provider_partial" | "estimated") => complete = false,
            _ => return LearningUsageCompleteness::Missing,
        }
    }
    if complete {
        LearningUsageCompleteness::Complete
    } else {
        LearningUsageCompleteness::Partial
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LearningResourceUsage {
    pub(crate) completeness: LearningUsageCompleteness,
    pub(crate) total_tokens: u64,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn learning_lineage_usage_from_metadata(
    metadata: &Metadata,
) -> Option<LearningResourceUsage> {
    let attempts = metadata
        .get("run_lineage_physical_model_attempts")?
        .parse::<u64>()
        .ok()?;
    let provider = metadata
        .get("run_lineage_provider_usage_attempts")?
        .parse::<u64>()
        .ok()?;
    let partial = metadata
        .get("run_lineage_partial_usage_attempts")?
        .parse::<u64>()
        .ok()?;
    let estimated = metadata
        .get("run_lineage_estimated_usage_attempts")?
        .parse::<u64>()
        .ok()?;
    let unknown = metadata
        .get("run_lineage_unknown_usage_attempts")?
        .parse::<u64>()
        .ok()?;
    let total_tokens = metadata
        .get("run_lineage_total_tokens")?
        .parse::<u64>()
        .ok()?;
    let prompt_tokens = metadata
        .get("run_lineage_prompt_tokens")?
        .parse::<u64>()
        .ok()?;
    let completion_tokens = metadata
        .get("run_lineage_completion_tokens")?
        .parse::<u64>()
        .ok()?;
    let source_attempts = provider
        .checked_add(partial)?
        .checked_add(estimated)?
        .checked_add(unknown)?;
    if source_attempts != attempts || prompt_tokens.checked_add(completion_tokens)? > total_tokens {
        return None;
    }
    let completeness = if attempts == 0 || unknown > 0 {
        LearningUsageCompleteness::Missing
    } else if partial > 0 || estimated > 0 {
        LearningUsageCompleteness::Partial
    } else {
        LearningUsageCompleteness::Complete
    };
    Some(LearningResourceUsage {
        completeness,
        total_tokens,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn routing_learning_evidence(
    run_events: &[&Event],
    decision: &Event,
    terminal: &Event,
    steer_epoch: Option<u64>,
) -> LearningEvidenceV1 {
    let resource_usage = learning_lineage_usage_from_metadata(&terminal.metadata);
    let usage = resource_usage
        .map(|usage| usage.completeness)
        .unwrap_or_else(|| learning_usage_completeness(run_events));
    let budget_fingerprint = learning_budget_fingerprint(&decision.metadata);
    let termination = match AgentRunEvent::from_event(terminal).map(AgentRunEvent::status) {
        Some(AgentRunStatus::Completed) => LearningTermination::Completed,
        Some(AgentRunStatus::Failed) => LearningTermination::Failed,
        Some(AgentRunStatus::Cancelled) => LearningTermination::Cancelled,
        _ => LearningTermination::Unknown,
    };
    let censored_attribution = match termination {
        LearningTermination::Cancelled => LearningAttribution::User,
        _ => LearningAttribution::Unknown,
    };
    let censored = || {
        LearningEvidenceV1::censored(
            termination,
            censored_attribution,
            usage,
            steer_epoch,
            budget_fingerprint.clone(),
        )
    };

    let (Some(steer_epoch), Some(budget_fingerprint)) = (steer_epoch, budget_fingerprint.clone())
    else {
        return censored();
    };
    if terminal_outcome_ledger_has_blocking_denial(
        terminal,
    ) {
        return censored();
    }
    if usage == LearningUsageCompleteness::Missing || termination != LearningTermination::Completed
    {
        return censored();
    }
    let Some(persisted) = LearningEvidenceV1::from_metadata(&terminal.metadata) else {
        return censored();
    };
    if resource_usage.is_none()
        || persisted.steer_epoch != Some(steer_epoch)
        || persisted.budget_fingerprint.as_deref() != Some(budget_fingerprint.as_str())
        || persisted.usage_completeness != usage
        || persisted.termination != termination
    {
        return censored();
    }
    persisted
}

#[cfg_attr(not(test), allow(dead_code))]
fn terminal_outcome_ledger_has_blocking_denial(terminal: &Event) -> bool {
    let Some(ledger) = agent_runtime::OutcomeLedgerShadow::from_terminal_metadata(
        &terminal.metadata,
        agent_runtime::OutcomeLedgerPhase::Completed,
    ) else {
        return false;
    };
    ledger.obligations.iter().any(|obligation| {
        obligation.satisfaction == agent_runtime::OutcomeSatisfaction::Blocked
            && obligation.blocker.is_some()
            && obligation.evidence_sequence.is_some_and(|sequence| {
                ledger.evidence.iter().any(|evidence| {
                    evidence.sequence == sequence
                        && evidence.kind == agent_runtime::ContractEvidenceKind::Denial
                })
            })
    })
}
