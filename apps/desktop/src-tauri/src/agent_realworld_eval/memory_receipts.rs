use super::Treatment;
use crate::sha256_hex;
use agent_core::{Event, Metadata};
use orchestrator::{AgentRunDecision, MemoryRecallPlan, MemoryRecallPolicy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum MemoryEffectCaseRole {
    Required,
    IrrelevantControl,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryEffectCaseContract {
    pub(super) role: MemoryEffectCaseRole,
}

pub(super) fn validate_memory_effect_suite(suite: &super::RealworldSuite) -> Result<(), String> {
    if suite.cases.len() != 3 || suite.default_replicates != 3 {
        return Err("memory-effect suite must freeze three cases and three replicates".to_string());
    }
    let roles = suite
        .cases
        .iter()
        .map(|case| {
            if case.seed_memory_prompt.as_deref().is_none_or(str::is_empty) {
                return Err(format!("{} must seed a durable memory fixture", case.id));
            }
            case.memory_effect
                .as_ref()
                .map(|contract| contract.role)
                .ok_or_else(|| format!("{} is missing its memory-effect role", case.id))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if roles
        .iter()
        .filter(|role| **role == MemoryEffectCaseRole::Required)
        .count()
        != 2
        || roles
            .iter()
            .filter(|role| **role == MemoryEffectCaseRole::IrrelevantControl)
            .count()
            != 1
    {
        return Err(
            "memory-effect suite must contain two required cases and one irrelevant control"
                .to_string(),
        );
    }
    let control = suite
        .cases
        .iter()
        .find(|case| {
            case.memory_effect
                .as_ref()
                .is_some_and(|contract| contract.role == MemoryEffectCaseRole::IrrelevantControl)
        })
        .expect("validated irrelevant control");
    if control.verification.output_not_contains.is_empty() {
        return Err("memory-effect irrelevant control must forbid at least one decoy".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MemoryEvaluationReceipt {
    pub(super) constraint: String,
    pub(super) routed_memory_policy: String,
    pub(super) effective_memory_policy: String,
    pub(super) recall_count: usize,
    pub(super) selected_count: usize,
    pub(super) routed_query_sha256: String,
    pub(super) selected_memory_ids_sha256: String,
    pub(super) non_memory_decision_sha256: String,
}

pub(super) fn memory_evaluation_receipt_from_events(
    events: &[Event],
    treatment: Treatment,
) -> Result<Option<MemoryEvaluationReceipt>, String> {
    if !treatment.is_memory_evaluation() {
        return Ok(None);
    }
    let decision_event = events
        .iter()
        .rev()
        .find(|event| event.summary == "Agent run decision selected")
        .ok_or_else(|| "memory evaluation routed decision receipt is missing".to_string())?;
    let decision_json = required_any(&decision_event.metadata, &["run_decision", "decision"])?;
    let decision = serde_json::from_str::<AgentRunDecision>(decision_json)
        .map_err(|error| format!("memory evaluation routed decision is invalid: {error}"))?;
    let execution_event = events
        .iter()
        .rev()
        .find(|event| event.summary == "Starting execution")
        .ok_or_else(|| "memory evaluation effective decision receipt is missing".to_string())?;
    let expected_constraint = treatment.label();
    let constraint = required(
        &execution_event.metadata,
        crate::agent_preparation_runtime::AGENT_MEMORY_EVALUATION_CONSTRAINT_KEY,
    )?;
    if constraint != expected_constraint {
        return Err(format!(
            "memory evaluation constraint drifted: expected {expected_constraint}, observed {constraint}"
        ));
    }
    let routed_memory_policy = required(
        &execution_event.metadata,
        crate::agent_preparation_runtime::ROUTED_MEMORY_POLICY_KEY,
    )?;
    let effective_memory_policy = required(
        &execution_event.metadata,
        crate::agent_preparation_runtime::EFFECTIVE_MEMORY_POLICY_KEY,
    )?;
    let expected_routed_policy = memory_policy_label(decision.memory.policy);
    if routed_memory_policy != expected_routed_policy {
        return Err(
            "memory evaluation routed policy does not match the routed decision".to_string(),
        );
    }
    if treatment.is_memory_off() {
        if effective_memory_policy != "none" {
            return Err("memory-off did not disable routed memory".to_string());
        }
    } else if effective_memory_policy != routed_memory_policy {
        return Err("memory-on changed the routed memory policy".to_string());
    }

    let recall_events = events
        .iter()
        .filter(|event| {
            event.summary == "Project memory recalled"
                && event.metadata.get("action").map(String::as_str) == Some("memory_recall")
        })
        .collect::<Vec<_>>();
    let selected_count = recall_events.iter().try_fold(0usize, |total, event| {
        required_usize(&event.metadata, "selected_count").map(|count| total.saturating_add(count))
    })?;
    let mut selected_ids = recall_events
        .iter()
        .flat_map(|event| {
            event
                .metadata
                .get("memory_ids")
                .into_iter()
                .flat_map(|value| value.split(','))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    selected_ids.sort();
    if selected_ids.len() != selected_count {
        return Err("memory evaluation selected-count receipt is inconsistent".to_string());
    }
    if treatment.is_memory_off() && (!recall_events.is_empty() || selected_count != 0) {
        return Err("memory-off observed a project memory recall".to_string());
    }

    let mut non_memory_decision = decision.clone();
    non_memory_decision.memory = MemoryRecallPlan::none();
    let non_memory_decision_json = serde_json::to_string(&non_memory_decision)
        .map_err(|error| format!("failed to encode non-memory decision receipt: {error}"))?;
    Ok(Some(MemoryEvaluationReceipt {
        constraint: constraint.to_string(),
        routed_memory_policy: routed_memory_policy.to_string(),
        effective_memory_policy: effective_memory_policy.to_string(),
        recall_count: recall_events.len(),
        selected_count,
        routed_query_sha256: domain_hash(
            "cindx.agent-memory-effect-routed-query.v1\0",
            &decision.memory.query,
        ),
        selected_memory_ids_sha256: domain_hash(
            "cindx.agent-memory-effect-selected-ids.v1\0",
            &selected_ids.join("\0"),
        ),
        non_memory_decision_sha256: domain_hash(
            "cindx.agent-memory-effect-non-memory-decision.v1\0",
            &non_memory_decision_json,
        ),
    }))
}

fn required<'a>(metadata: &'a Metadata, key: &str) -> Result<&'a str, String> {
    metadata
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("required memory receipt field {key} is missing"))
}

fn required_any<'a>(metadata: &'a Metadata, keys: &[&str]) -> Result<&'a str, String> {
    keys.iter()
        .find_map(|key| metadata.get(*key).map(String::as_str))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("required memory receipt fields {keys:?} are missing"))
}

fn required_usize(metadata: &Metadata, key: &str) -> Result<usize, String> {
    required(metadata, key)?
        .parse::<usize>()
        .map_err(|_| format!("required numeric memory receipt field {key} is invalid"))
}

fn memory_policy_label(policy: MemoryRecallPolicy) -> &'static str {
    match policy {
        MemoryRecallPolicy::None => "none",
        MemoryRecallPolicy::Relevant => "relevant",
        MemoryRecallPolicy::Comprehensive => "comprehensive",
    }
}

fn domain_hash(domain: &str, value: &str) -> String {
    sha256_hex(format!("{domain}{value}").as_bytes())
}
