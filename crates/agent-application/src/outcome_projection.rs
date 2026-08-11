use crate::outcome_evidence::{
    validate_sha256, AgentOutcomeEvidenceError, AgentOutcomeExposureV1,
    AgentOutcomeLifecycleBindingV1, AgentOutcomeTerminalResourcesV1, AgentOutcomeTerminalStatusV1,
    AgentOutcomeUsageV1,
};
use crate::{
    AgentRunEvent, AgentRunStatus, AgentStrategyReceiptState, AgentTerminalCommitIdentity,
    AgentTerminalCommitState, AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY,
    AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY, AGENT_TERMINAL_COMMIT_SCHEMA,
    AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY,
};
use agent_core::{
    decode_event_type, DecodedEventType, Event, EventKind, EventTypeV1, Metadata,
    AGENT_ACTOR_METADATA_KEY, AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY,
    AGENT_ATTRIBUTION_LEGACY_ROLE_METADATA_KEY, AGENT_ATTRIBUTION_MODEL_METADATA_KEY,
    AGENT_EFFECT_AUTHORITY_METADATA_KEY, AGENT_MODEL_ATTRIBUTION_SCHEMA,
    AGENT_MODEL_ATTRIBUTION_SCHEMA_METADATA_KEY, AGENT_MODEL_PROFILE_METADATA_KEY,
    AGENT_OUTPUT_TRUST_METADATA_KEY, AGENT_SERVICE_METADATA_KEY, AGENT_STAGE_METADATA_KEY,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelAttributionReceipt {
    actor: String,
    service: String,
    stage: String,
    model_profile: String,
    output_trust: String,
    effect_authority: String,
    component: String,
    model: String,
    legacy_role: String,
    collaboration_id: Option<String>,
}

#[derive(Debug, Clone)]
struct StartedModelCall {
    sequence: u64,
    attribution: ModelAttributionReceipt,
}

impl AgentOutcomeTerminalStatusV1 {
    fn from_status(status: AgentRunStatus) -> Result<Self, AgentOutcomeEvidenceError> {
        match status {
            AgentRunStatus::Completed => Ok(Self::Completed),
            AgentRunStatus::Failed => Ok(Self::Failed),
            AgentRunStatus::Cancelled => Ok(Self::Cancelled),
            _ => Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome requires a terminal run",
            )),
        }
    }
}

impl AgentOutcomeLifecycleBindingV1 {
    pub fn from_events(events: &[Event]) -> Result<Self, AgentOutcomeEvidenceError> {
        let mut terminal_events = Vec::new();
        for event in events {
            let lifecycle = AgentRunEvent::try_from_event(event).map_err(|error| {
                AgentOutcomeEvidenceError::new(format!(
                    "externally verified outcome lifecycle is invalid: {error}"
                ))
            })?;
            if lifecycle.is_some_and(|event| event.status().is_terminal()) {
                terminal_events.push(event);
            }
        }
        let [terminal] = terminal_events.as_slice() else {
            return Err(AgentOutcomeEvidenceError::new(
                if terminal_events.is_empty() {
                    "externally verified outcome terminal receipt is missing"
                } else {
                    "externally verified outcome has duplicate terminal receipts"
                },
            ));
        };
        if terminal
            .metadata
            .get(AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY)
            .map(String::as_str)
            != Some(AGENT_TERMINAL_COMMIT_SCHEMA)
        {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome terminal receipt schema is missing",
            ));
        }
        let steer_epoch = terminal
            .metadata
            .get(AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY)
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                AgentOutcomeEvidenceError::new(
                    "externally verified outcome terminal epoch is invalid",
                )
            })?;
        let strategy_state =
            AgentStrategyReceiptState::from_metadata(&terminal.metadata).map_err(|error| {
                AgentOutcomeEvidenceError::new(format!(
                    "externally verified outcome strategy receipt is invalid: {error}"
                ))
            })?;
        if strategy_state == AgentStrategyReceiptState::Absent {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome strategy receipt is missing",
            ));
        }
        let identity =
            AgentTerminalCommitIdentity::new(&terminal.task_id, &terminal.metadata, steer_epoch)
                .map_err(|error| {
                    AgentOutcomeEvidenceError::new(format!(
                        "externally verified outcome terminal identity is invalid: {error}"
                    ))
                })?;
        match identity.inspect_events(events) {
            Ok(AgentTerminalCommitState::Committed) => {}
            Ok(AgentTerminalCommitState::Pending) => {
                return Err(AgentOutcomeEvidenceError::new(
                    "externally verified outcome terminal receipt is missing",
                ))
            }
            Err(error) => {
                return Err(AgentOutcomeEvidenceError::new(format!(
                    "externally verified outcome terminal receipt is invalid: {error}"
                )))
            }
        }
        let strategy = match strategy_state {
            AgentStrategyReceiptState::Selected(strategy) => strategy,
            AgentStrategyReceiptState::NotSelected => {
                return Err(AgentOutcomeEvidenceError::new(
                    "externally verified outcome is censored before strategy decision because no strategy was selected",
                ))
            }
            AgentStrategyReceiptState::Absent => {
                return Err(AgentOutcomeEvidenceError::new(
                    "externally verified outcome strategy receipt is missing",
                ))
            }
        };
        let decisions = events
            .iter()
            .filter(|event| {
                event.metadata.get("agent_run_id").map(String::as_str)
                    == Some(strategy.agent_run_id())
                    && event
                        .metadata
                        .get("steer_epoch")
                        .and_then(|value| value.parse::<u64>().ok())
                        == Some(strategy.steer_epoch())
                    && matches!(
                        decode_event_type(event),
                        DecodedEventType::V1(typed)
                            if typed.event_type() == EventTypeV1::AgentRunDecisionSelected
                    )
            })
            .collect::<Vec<_>>();
        let [decision] = decisions.as_slice() else {
            return Err(AgentOutcomeEvidenceError::new(if decisions.is_empty() {
                "externally verified outcome strategy decision is missing"
            } else {
                "externally verified outcome has duplicate strategy decisions"
            }));
        };
        if !strategy.matches_decision_event(decision) {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome strategy receipt does not match its decision",
            ));
        }
        if terminal.sequence <= decision.sequence {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome terminal precedes its strategy decision",
            ));
        }
        let semantic_plan = required_sha256(
            &decision.metadata,
            "execution_plan_semantic_sha256",
            "semantic execution plan",
        )?;
        let terminal_commit_key = required_sha256(
            &terminal.metadata,
            AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY,
            "terminal commit key",
        )?;
        let terminal_status = AgentRunEvent::try_from_event(terminal)
            .map_err(|error| AgentOutcomeEvidenceError::new(error.to_string()))?
            .ok_or_else(|| {
                AgentOutcomeEvidenceError::new(
                    "externally verified outcome terminal event is invalid",
                )
            })?
            .status();
        let binding = Self {
            agent_run_id: strategy.agent_run_id().to_string(),
            steer_epoch,
            strategy_receipt_key: strategy.key().to_string(),
            strategy_plan_sha256: strategy.plan_sha256().to_string(),
            execution_plan_semantic_sha256: semantic_plan,
            terminal_commit_key,
            terminal_status: AgentOutcomeTerminalStatusV1::from_status(terminal_status)?,
            decision_sequence: decision.sequence,
            terminal_sequence: terminal.sequence,
        };
        binding.validate()?;
        Ok(binding)
    }
}

impl AgentOutcomeExposureV1 {
    pub fn from_events(
        events: &[Event],
        lifecycle: &AgentOutcomeLifecycleBindingV1,
    ) -> Result<Self, AgentOutcomeEvidenceError> {
        lifecycle.validate()?;
        let scoped = events
            .iter()
            .filter(|event| event_in_lifecycle(event, lifecycle))
            .collect::<Vec<_>>();

        let workflow_events = scoped
            .iter()
            .copied()
            .filter(|event| {
                matches!(
                    event.summary.as_str(),
                    "Collaboration workflow planned" | "Collaboration workflow completed"
                )
            })
            .collect::<Vec<_>>();
        if workflow_events
            .iter()
            .any(|event| event.sequence >= lifecycle.terminal_sequence)
        {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome has workflow evidence after terminal",
            ));
        }
        let planned = workflow_events
            .iter()
            .copied()
            .filter(|event| {
                event.sequence > lifecycle.decision_sequence
                    && event.summary == "Collaboration workflow planned"
            })
            .collect::<Vec<_>>();
        if planned.len() > 1 {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome has duplicate workflow plans",
            ));
        }
        let planned_workflow = planned
            .first()
            .map(|event| {
                required_metadata(&event.metadata, "collaboration_id", "collaboration id")
                    .map(|collaboration_id| (event.sequence, collaboration_id.to_string()))
            })
            .transpose()?;
        let completed = workflow_events
            .iter()
            .copied()
            .filter(|event| event.summary == "Collaboration workflow completed")
            .collect::<Vec<_>>();
        if completed.len() > 1 {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome has duplicate workflow completions",
            ));
        }
        let workflow_completed = match (planned_workflow.as_ref(), completed.first()) {
            (None, Some(_)) => {
                return Err(AgentOutcomeEvidenceError::new(
                    "externally verified outcome has an orphan workflow completion",
                ))
            }
            (Some((planned_sequence, collaboration_id)), Some(completed)) => {
                let completed_id =
                    required_metadata(&completed.metadata, "collaboration_id", "collaboration id")?;
                if completed.sequence <= *planned_sequence || completed_id != collaboration_id {
                    return Err(AgentOutcomeEvidenceError::new(
                        "externally verified outcome workflow completion is inconsistent",
                    ));
                }
                true
            }
            _ => false,
        };
        let collaboration_id = planned_workflow
            .as_ref()
            .map(|(_, collaboration_id)| collaboration_id.as_str());

        let mut starts = BTreeMap::new();
        for event in scoped
            .iter()
            .copied()
            .filter(|event| event.kind == EventKind::ModelRequestStarted)
        {
            validate_model_event_type(event, EventTypeV1::AgentModelTurnStarted)?;
            if event.sequence >= lifecycle.terminal_sequence {
                return Err(AgentOutcomeEvidenceError::new(
                    "externally verified outcome has a model call after terminal",
                ));
            }
            let request_id =
                required_metadata(&event.metadata, "request_id", "model request id")?.to_string();
            let call = StartedModelCall {
                sequence: event.sequence,
                attribution: model_attribution_receipt(event)?,
            };
            if starts.insert(request_id.clone(), call).is_some() {
                return Err(AgentOutcomeEvidenceError::new(format!(
                    "externally verified outcome has duplicate model start {request_id}"
                )));
            }
        }
        let mut receipt = Self {
            workflow_planned: planned_workflow.is_some(),
            workflow_completed,
            ..Self::default()
        };
        for call in starts.values() {
            if matches!(
                call.attribution.actor.as_str(),
                "specialist" | "independent_verifier"
            ) {
                receipt.worker_model_calls = receipt.worker_model_calls.saturating_add(1);
                receipt.worker_models.insert(call.attribution.model.clone());
            }
            if call.attribution.component.starts_with("direct_anchor") {
                receipt.direct_anchor_competition_calls =
                    receipt.direct_anchor_competition_calls.saturating_add(1);
            }
            if call.attribution.effect_authority == "permission_gated"
                && call.attribution.actor != "owner"
            {
                receipt.non_owner_permission_gated_calls =
                    receipt.non_owner_permission_gated_calls.saturating_add(1);
            }
        }

        let mut finished = BTreeSet::new();
        let mut successful_responses = 0usize;
        for event in scoped
            .iter()
            .copied()
            .filter(|event| event.kind == EventKind::ModelRequestFinished)
        {
            validate_model_event_type(event, EventTypeV1::AgentModelTurnFinished)?;
            if event.sequence >= lifecycle.terminal_sequence {
                return Err(AgentOutcomeEvidenceError::new(
                    "externally verified outcome has a model result after terminal",
                ));
            }
            let request_id = required_metadata(&event.metadata, "request_id", "model request id")?;
            let started = starts.get(request_id).ok_or_else(|| {
                AgentOutcomeEvidenceError::new(format!(
                    "externally verified outcome has orphan model finish {request_id}"
                ))
            })?;
            if !finished.insert(request_id.to_string()) {
                return Err(AgentOutcomeEvidenceError::new(format!(
                    "externally verified outcome has duplicate model finish {request_id}"
                )));
            }
            if event.sequence <= started.sequence {
                return Err(AgentOutcomeEvidenceError::new(format!(
                    "externally verified outcome model finish precedes start {request_id}"
                )));
            }
            let finished_attribution = model_attribution_receipt(event)?;
            if finished_attribution != started.attribution {
                return Err(AgentOutcomeEvidenceError::new(format!(
                    "externally verified outcome attribution changed for {request_id}"
                )));
            }
            let response_count = successful_response_count(event)?;
            successful_responses = successful_responses.saturating_add(response_count);
            if response_count == 0 {
                continue;
            }
            let attribution = &started.attribution;
            match attribution.actor.as_str() {
                "owner" => {
                    receipt.successful_owner_model_calls =
                        receipt.successful_owner_model_calls.saturating_add(1)
                }
                "specialist" => {
                    receipt.successful_specialist_model_calls =
                        receipt.successful_specialist_model_calls.saturating_add(1)
                }
                "independent_verifier" => {
                    receipt.successful_independent_verifier_model_calls = receipt
                        .successful_independent_verifier_model_calls
                        .saturating_add(1)
                }
                _ => {}
            }
            if attribution.service == "conductor" {
                receipt.successful_conductor_model_calls =
                    receipt.successful_conductor_model_calls.saturating_add(1);
            }
            let workflow_step = collaboration_id.is_some_and(|collaboration_id| {
                attribution.collaboration_id.as_deref() == Some(collaboration_id)
                    && attribution.effect_authority == "read_only"
                    && !attribution.component.starts_with("direct_anchor")
            });
            if workflow_step {
                match attribution.actor.as_str() {
                    "specialist" => {
                        receipt.successful_workflow_specialist_model_calls = receipt
                            .successful_workflow_specialist_model_calls
                            .saturating_add(1);
                        receipt
                            .successful_workflow_specialist_models
                            .insert(attribution.model.clone());
                    }
                    "independent_verifier" => {
                        receipt.successful_workflow_verifier_model_calls = receipt
                            .successful_workflow_verifier_model_calls
                            .saturating_add(1);
                        receipt
                            .successful_workflow_verifier_models
                            .insert(attribution.model.clone());
                    }
                    _ => {}
                }
            }
        }
        if finished.len() != starts.len() {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome has an unfinished model call",
            ));
        }
        receipt.logical_model_calls = starts.len().max(successful_responses);
        receipt.validate()?;
        Ok(receipt)
    }
}

impl AgentOutcomeUsageV1 {
    fn from_metadata(metadata: &Metadata, prefix: &str) -> Result<Self, AgentOutcomeEvidenceError> {
        let read = |suffix: &str| {
            metadata
                .get(&format!("{prefix}_{suffix}"))
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| {
                    AgentOutcomeEvidenceError::new(format!(
                        "externally verified outcome resource field {prefix}_{suffix} is missing"
                    ))
                })
        };
        let usage = Self {
            physical_model_attempts: read("physical_model_attempts")?,
            prompt_tokens: read("prompt_tokens")?,
            completion_tokens: read("completion_tokens")?,
            total_tokens: read("total_tokens")?,
            reserved_tokens: read("reserved_tokens")?,
            provider_usage_attempts: read("provider_usage_attempts")?,
            partial_usage_attempts: read("partial_usage_attempts")?,
            estimated_usage_attempts: read("estimated_usage_attempts")?,
            unknown_usage_attempts: read("unknown_usage_attempts")?,
        };
        usage.validate()?;
        Ok(usage)
    }
}

impl AgentOutcomeTerminalResourcesV1 {
    pub fn from_events(
        events: &[Event],
        lifecycle: &AgentOutcomeLifecycleBindingV1,
    ) -> Result<Self, AgentOutcomeEvidenceError> {
        lifecycle.validate()?;
        let terminals = events
            .iter()
            .filter(|event| {
                event.sequence == lifecycle.terminal_sequence
                    && event_in_lifecycle(event, lifecycle)
                    && event
                        .metadata
                        .get(AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY)
                        .map(String::as_str)
                        == Some(lifecycle.terminal_commit_key.as_str())
            })
            .collect::<Vec<_>>();
        let [terminal] = terminals.as_slice() else {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome resource terminal is missing",
            ));
        };
        let status = AgentRunEvent::try_from_event(terminal)
            .map_err(|error| AgentOutcomeEvidenceError::new(error.to_string()))?
            .map(|event| event.status())
            .ok_or_else(|| {
                AgentOutcomeEvidenceError::new(
                    "externally verified outcome resource terminal is invalid",
                )
            })?;
        if AgentOutcomeTerminalStatusV1::from_status(status)? != lifecycle.terminal_status {
            return Err(AgentOutcomeEvidenceError::new(
                "externally verified outcome resource terminal status changed",
            ));
        }
        let resources = Self {
            segment: AgentOutcomeUsageV1::from_metadata(&terminal.metadata, "run_segment")?,
            lineage: AgentOutcomeUsageV1::from_metadata(&terminal.metadata, "run_lineage")?,
        };
        resources.validate()?;
        Ok(resources)
    }
}

fn event_in_lifecycle(event: &Event, lifecycle: &AgentOutcomeLifecycleBindingV1) -> bool {
    event.metadata.get("agent_run_id").map(String::as_str) == Some(lifecycle.agent_run_id.as_str())
        && event
            .metadata
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            == Some(lifecycle.steer_epoch)
}

fn validate_model_event_type(
    event: &Event,
    expected: EventTypeV1,
) -> Result<(), AgentOutcomeEvidenceError> {
    if !matches!(
        decode_event_type(event),
        DecodedEventType::V1(typed) if typed.event_type() == expected
    ) {
        return Err(AgentOutcomeEvidenceError::new(
            "externally verified outcome model lifecycle type is invalid",
        ));
    }
    Ok(())
}

fn model_attribution_receipt(
    event: &Event,
) -> Result<ModelAttributionReceipt, AgentOutcomeEvidenceError> {
    if event
        .metadata
        .get(AGENT_MODEL_ATTRIBUTION_SCHEMA_METADATA_KEY)
        .map(String::as_str)
        != Some(AGENT_MODEL_ATTRIBUTION_SCHEMA)
    {
        return Err(AgentOutcomeEvidenceError::new(
            "externally verified outcome model attribution schema is missing",
        ));
    }
    let actor = required_metadata(&event.metadata, AGENT_ACTOR_METADATA_KEY, "actor")?;
    let service = required_metadata(&event.metadata, AGENT_SERVICE_METADATA_KEY, "service")?;
    let stage = required_metadata(&event.metadata, AGENT_STAGE_METADATA_KEY, "stage")?;
    let profile = required_metadata(
        &event.metadata,
        AGENT_MODEL_PROFILE_METADATA_KEY,
        "model profile",
    )?;
    let output_trust = required_metadata(
        &event.metadata,
        AGENT_OUTPUT_TRUST_METADATA_KEY,
        "output trust",
    )?;
    let effect_authority = required_metadata(
        &event.metadata,
        AGENT_EFFECT_AUTHORITY_METADATA_KEY,
        "effect authority",
    )?;
    let component = required_metadata(
        &event.metadata,
        AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY,
        "attribution component",
    )?;
    let model = required_metadata(
        &event.metadata,
        AGENT_ATTRIBUTION_MODEL_METADATA_KEY,
        "attribution model",
    )?;
    let legacy_role = required_metadata(
        &event.metadata,
        AGENT_ATTRIBUTION_LEGACY_ROLE_METADATA_KEY,
        "legacy attribution role",
    )?;
    if !matches!(
        actor,
        "none" | "owner" | "specialist" | "independent_verifier"
    ) || !matches!(service, "none" | "conductor" | "learning_utility")
        || (actor == "none") == (service == "none")
        || !matches!(stage, "plan" | "evidence" | "act" | "verify" | "finalize")
        || !matches!(profile, "primary" | "reasoning" | "verifier" | "utility")
        || output_trust != "untrusted_model_output"
        || !matches!(effect_authority, "none" | "read_only" | "permission_gated")
    {
        return Err(AgentOutcomeEvidenceError::new(
            "externally verified outcome model attribution is invalid",
        ));
    }
    Ok(ModelAttributionReceipt {
        actor: actor.to_string(),
        service: service.to_string(),
        stage: stage.to_string(),
        model_profile: profile.to_string(),
        output_trust: output_trust.to_string(),
        effect_authority: effect_authority.to_string(),
        component: component.to_string(),
        model: model.to_string(),
        legacy_role: legacy_role.to_string(),
        collaboration_id: event.metadata.get("collaboration_id").cloned(),
    })
}

fn successful_response_count(event: &Event) -> Result<usize, AgentOutcomeEvidenceError> {
    if let Some(raw) = event.metadata.get("worker_model_responses") {
        return raw.parse::<usize>().map_err(|_| {
            AgentOutcomeEvidenceError::new(
                "externally verified outcome worker response count is invalid",
            )
        });
    }
    if event.metadata.contains_key("request_payload_sha256") {
        return Ok(1);
    }
    Ok(usize::from(
        event.summary == "Agent model turn finished"
            || event.metadata.get("status").map(String::as_str) == Some("completed")
            || event.metadata.contains_key("output"),
    ))
}

fn required_metadata<'a>(
    metadata: &'a Metadata,
    key: &str,
    label: &str,
) -> Result<&'a str, AgentOutcomeEvidenceError> {
    metadata
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AgentOutcomeEvidenceError::new(format!(
                "externally verified outcome {label} is missing"
            ))
        })
}

fn required_sha256(
    metadata: &Metadata,
    key: &str,
    label: &str,
) -> Result<String, AgentOutcomeEvidenceError> {
    let value = required_metadata(metadata, key, label)?;
    validate_sha256(value, label)?;
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentExternalPostconditionV1, AgentExternalVerifierV1, AgentOutcomeDispositionV1,
        AgentOutcomeResourcesV1, AgentStrategyDecisionReceipt, ExternallyVerifiedOutcomeV1,
        EXTERNALLY_VERIFIED_OUTCOME_SCHEMA,
    };
    use agent_core::{
        insert_event_type_v1, AgentActor, AgentEffectAuthority, AgentModelAttribution,
        AgentModelProfile, AgentStage, EventId, TaskId, EVENT_TYPE_METADATA_KEY,
    };

    fn event(sequence: u64, kind: EventKind, event_type: EventTypeV1, summary: &str) -> Event {
        let mut event = Event {
            id: EventId(format!("event-{sequence}")),
            task_id: TaskId("task".to_string()),
            sequence,
            timestamp_ms: sequence,
            kind,
            summary: summary.to_string(),
            metadata: [
                ("session_id".to_string(), "session".to_string()),
                ("agent_run_id".to_string(), "run".to_string()),
                ("steer_epoch".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        insert_event_type_v1(&event.kind, &mut event.metadata, event_type).unwrap();
        event
    }

    fn usage_metadata(attempts: u64, unknown: u64) -> Metadata {
        let mut metadata = Metadata::new();
        for prefix in ["run_segment", "run_lineage"] {
            metadata.insert(
                format!("{prefix}_physical_model_attempts"),
                attempts.to_string(),
            );
            metadata.insert(format!("{prefix}_prompt_tokens"), "10".to_string());
            metadata.insert(format!("{prefix}_completion_tokens"), "5".to_string());
            metadata.insert(format!("{prefix}_total_tokens"), "15".to_string());
            metadata.insert(format!("{prefix}_reserved_tokens"), "0".to_string());
            metadata.insert(
                format!("{prefix}_provider_usage_attempts"),
                attempts.saturating_sub(unknown).to_string(),
            );
            metadata.insert(format!("{prefix}_partial_usage_attempts"), "0".to_string());
            metadata.insert(
                format!("{prefix}_estimated_usage_attempts"),
                "0".to_string(),
            );
            metadata.insert(
                format!("{prefix}_unknown_usage_attempts"),
                unknown.to_string(),
            );
        }
        metadata
    }

    fn lifecycle_events(status: AgentOutcomeTerminalStatusV1) -> Vec<Event> {
        let plan = "a".repeat(64);
        let mut decision = event(
            2,
            EventKind::TaskStatusChanged,
            EventTypeV1::AgentRunDecisionSelected,
            "Agent run decision selected",
        );
        decision
            .metadata
            .insert("execution_plan_semantic_sha256".to_string(), plan.clone());
        let strategy =
            AgentStrategyDecisionReceipt::new(&decision.task_id, &decision.metadata, &plan)
                .unwrap();
        strategy.insert_into(&mut decision.metadata).unwrap();
        let (event_type, kind, summary) = match status {
            AgentOutcomeTerminalStatusV1::Completed => (
                EventTypeV1::AgentRunCompleted,
                EventKind::TaskStatusChanged,
                "Agent task completed",
            ),
            AgentOutcomeTerminalStatusV1::Failed => (
                EventTypeV1::AgentRunFailed,
                EventKind::Error,
                "Agent task failed",
            ),
            AgentOutcomeTerminalStatusV1::Cancelled => (
                EventTypeV1::AgentRunCancelled,
                EventKind::TaskStatusChanged,
                "Agent task cancelled",
            ),
        };
        let mut terminal = event(5, kind, event_type, summary);
        strategy.insert_into(&mut terminal.metadata).unwrap();
        terminal.metadata.extend(usage_metadata(1, 0));
        let terminal_identity =
            AgentTerminalCommitIdentity::new(&terminal.task_id, &terminal.metadata, 0).unwrap();
        terminal.metadata.extend(terminal_identity.metadata());
        vec![decision, terminal]
    }

    fn pre_decision_terminal_event() -> Event {
        let mut terminal = event(
            5,
            EventKind::Error,
            EventTypeV1::AgentRunFailed,
            "Agent task failed",
        );
        crate::insert_strategy_not_selected(&mut terminal.metadata, 0).unwrap();
        let terminal_identity =
            AgentTerminalCommitIdentity::new(&terminal.task_id, &terminal.metadata, 0).unwrap();
        terminal.metadata.extend(terminal_identity.metadata());
        terminal
    }

    fn model_call(sequence: u64, actor: AgentActor, model: &str) -> (Event, Event) {
        let mut started = event(
            sequence,
            EventKind::ModelRequestStarted,
            EventTypeV1::AgentModelTurnStarted,
            "Agent model turn started",
        );
        started
            .metadata
            .insert("request_id".to_string(), format!("request-{sequence}"));
        AgentModelAttribution::actor(
            actor,
            AgentStage::Act,
            AgentModelProfile::Primary,
            if actor == AgentActor::Owner {
                AgentEffectAuthority::PermissionGated
            } else {
                AgentEffectAuthority::ReadOnly
            },
        )
        .insert_into(&mut started.metadata, model, "executor", "foreground")
        .unwrap();
        let mut finished = started.clone();
        finished.id = EventId(format!("event-{}", sequence + 1));
        finished.sequence += 1;
        finished.kind = EventKind::ModelRequestFinished;
        finished.summary = "Agent model turn finished".to_string();
        finished.metadata.remove(EVENT_TYPE_METADATA_KEY);
        insert_event_type_v1(
            &finished.kind,
            &mut finished.metadata,
            EventTypeV1::AgentModelTurnFinished,
        )
        .unwrap();
        finished
            .metadata
            .insert("status".to_string(), "completed".to_string());
        (started, finished)
    }

    fn lifecycle_exposure_resources(
        terminal_status: AgentOutcomeTerminalStatusV1,
    ) -> (
        AgentOutcomeLifecycleBindingV1,
        AgentOutcomeExposureV1,
        AgentOutcomeTerminalResourcesV1,
    ) {
        let mut events = lifecycle_events(terminal_status);
        let (started, finished) = model_call(3, AgentActor::Owner, "model");
        events.insert(1, started);
        events.insert(2, finished);
        let lifecycle = AgentOutcomeLifecycleBindingV1::from_events(&events).unwrap();
        let exposure = AgentOutcomeExposureV1::from_events(&events, &lifecycle).unwrap();
        let resources = AgentOutcomeTerminalResourcesV1::from_events(&events, &lifecycle).unwrap();
        (lifecycle, exposure, resources)
    }

    fn check(passed: bool, preservation: bool, seed: char) -> AgentExternalPostconditionV1 {
        AgentExternalPostconditionV1 {
            kind: if preservation {
                "immutable_fixture"
            } else {
                "exact_file"
            }
            .to_string(),
            subject_sha256: seed.to_string().repeat(64),
            expected_sha256: "d".repeat(64),
            observed_sha256: Some("e".repeat(64)),
            artifact_sha256: Some("f".repeat(64)),
            bytes: Some(1),
            passed,
            preservation,
        }
    }

    fn outcome(
        terminal_status: AgentOutcomeTerminalStatusV1,
        safety_violations: usize,
        checks: Vec<AgentExternalPostconditionV1>,
    ) -> Result<ExternallyVerifiedOutcomeV1, AgentOutcomeEvidenceError> {
        let (lifecycle, exposure, terminal) = lifecycle_exposure_resources(terminal_status);
        let resources = AgentOutcomeResourcesV1::new(
            "1".repeat(64),
            "2".repeat(64),
            "3".repeat(64),
            10,
            exposure.logical_model_calls,
            0,
            terminal,
        )?;
        ExternallyVerifiedOutcomeV1::new(
            lifecycle,
            exposure,
            AgentExternalVerifierV1 {
                kind: "frozen_postconditions".to_string(),
                protocol_sha256: "4".repeat(64),
                subject_sha256: "5".repeat(64),
                safety_violations,
            },
            checks,
            resources,
        )
    }

    #[test]
    fn agent_outcome_evidence_contract_binds_terminal_strategy_and_semantic_plan() {
        let events = lifecycle_events(AgentOutcomeTerminalStatusV1::Completed);
        let lifecycle = AgentOutcomeLifecycleBindingV1::from_events(&events).unwrap();
        assert_eq!(lifecycle.agent_run_id, "run");
        assert_eq!(lifecycle.execution_plan_semantic_sha256, "a".repeat(64));
        assert_eq!(lifecycle.decision_sequence, 2);
        assert_eq!(lifecycle.terminal_sequence, 5);
        println!("{EXTERNALLY_VERIFIED_OUTCOME_SCHEMA}");
    }

    #[test]
    fn agent_outcome_evidence_contract_censors_valid_pre_decision_terminal_after_identity_check() {
        let terminal = pre_decision_terminal_event();
        let error = AgentOutcomeLifecycleBindingV1::from_events(std::slice::from_ref(&terminal))
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "externally verified outcome is censored before strategy decision because no strategy was selected"
        );

        let mut tampered = terminal;
        tampered.metadata.insert(
            AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY.to_string(),
            "9".repeat(64),
        );
        let error = AgentOutcomeLifecycleBindingV1::from_events(&[tampered])
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("externally verified outcome terminal receipt is invalid:"));
    }

    #[test]
    fn agent_outcome_evidence_contract_projects_actual_actor_exposure() {
        let (lifecycle, exposure, _) =
            lifecycle_exposure_resources(AgentOutcomeTerminalStatusV1::Completed);
        assert_eq!(lifecycle.steer_epoch, 0);
        assert_eq!(exposure.logical_model_calls, 1);
        assert_eq!(exposure.successful_owner_model_calls, 1);
        assert_eq!(exposure.worker_model_calls, 0);
        let mut impossible = exposure.clone();
        impossible.successful_specialist_model_calls = 1;
        assert!(impossible.validate().is_err());

        let mut malformed = lifecycle_events(AgentOutcomeTerminalStatusV1::Completed);
        let (started, mut finished) = model_call(3, AgentActor::Owner, "model");
        finished
            .metadata
            .insert("worker_model_responses".to_string(), "invalid".to_string());
        malformed.insert(1, started);
        malformed.insert(2, finished);
        let lifecycle = AgentOutcomeLifecycleBindingV1::from_events(&malformed).unwrap();
        assert!(AgentOutcomeExposureV1::from_events(&malformed, &lifecycle).is_err());

        let mut changed_attribution = lifecycle_events(AgentOutcomeTerminalStatusV1::Completed);
        let (started, mut finished) = model_call(3, AgentActor::Owner, "model");
        finished
            .metadata
            .insert(AGENT_STAGE_METADATA_KEY.to_string(), "verify".to_string());
        changed_attribution.insert(1, started);
        changed_attribution.insert(2, finished);
        let lifecycle = AgentOutcomeLifecycleBindingV1::from_events(&changed_attribution).unwrap();
        assert!(AgentOutcomeExposureV1::from_events(&changed_attribution, &lifecycle).is_err());
    }

    #[test]
    fn agent_outcome_evidence_contract_scores_positive_partial_and_negative() {
        let positive = outcome(
            AgentOutcomeTerminalStatusV1::Completed,
            0,
            vec![check(true, false, '6')],
        )
        .unwrap();
        let partial = outcome(
            AgentOutcomeTerminalStatusV1::Completed,
            0,
            vec![check(true, false, '6'), check(false, false, '7')],
        )
        .unwrap();
        let negative = outcome(
            AgentOutcomeTerminalStatusV1::Completed,
            0,
            vec![check(false, false, '6')],
        )
        .unwrap();
        assert_eq!(positive.reward_bps().unwrap(), 10_000);
        assert_eq!(
            positive.disposition().unwrap(),
            AgentOutcomeDispositionV1::Positive
        );
        assert_eq!(partial.reward_bps().unwrap(), 5_000);
        assert_eq!(
            partial.disposition().unwrap(),
            AgentOutcomeDispositionV1::Partial
        );
        assert_eq!(negative.reward_bps().unwrap(), 0);
        assert_eq!(
            negative.disposition().unwrap(),
            AgentOutcomeDispositionV1::Negative
        );
    }

    #[test]
    fn agent_outcome_evidence_contract_keeps_valid_failures_in_denominator() {
        let failed = outcome(
            AgentOutcomeTerminalStatusV1::Failed,
            0,
            vec![check(true, false, '6'), check(true, false, '7')],
        )
        .unwrap();
        assert_eq!(failed.reward_fraction().unwrap(), (0, 2));
        assert_eq!(
            failed.disposition().unwrap(),
            AgentOutcomeDispositionV1::Negative
        );
    }

    #[test]
    fn agent_outcome_evidence_contract_zeros_safety_or_preservation_failures() {
        let unsafe_outcome = outcome(
            AgentOutcomeTerminalStatusV1::Completed,
            1,
            vec![check(true, false, '6')],
        )
        .unwrap();
        let preservation_failure = outcome(
            AgentOutcomeTerminalStatusV1::Completed,
            0,
            vec![check(true, false, '6'), check(false, true, '7')],
        )
        .unwrap();
        assert_eq!(unsafe_outcome.reward_bps().unwrap(), 0);
        assert_eq!(preservation_failure.reward_bps().unwrap(), 0);
    }

    #[test]
    fn agent_outcome_evidence_contract_rejects_missing_resource_usage() {
        let mut events = lifecycle_events(AgentOutcomeTerminalStatusV1::Completed);
        events[1].metadata.insert(
            "run_lineage_unknown_usage_attempts".to_string(),
            "1".to_string(),
        );
        events[1].metadata.insert(
            "run_lineage_provider_usage_attempts".to_string(),
            "0".to_string(),
        );
        let lifecycle = AgentOutcomeLifecycleBindingV1::from_events(&events).unwrap();
        assert!(AgentOutcomeTerminalResourcesV1::from_events(&events, &lifecycle).is_err());

        let mut partial = lifecycle_events(AgentOutcomeTerminalStatusV1::Completed);
        for prefix in ["run_segment", "run_lineage"] {
            partial[1]
                .metadata
                .insert(format!("{prefix}_provider_usage_attempts"), "0".to_string());
            partial[1]
                .metadata
                .insert(format!("{prefix}_partial_usage_attempts"), "1".to_string());
        }
        let lifecycle = AgentOutcomeLifecycleBindingV1::from_events(&partial).unwrap();
        assert!(AgentOutcomeTerminalResourcesV1::from_events(&partial, &lifecycle).is_ok());

        let overflowed = AgentOutcomeUsageV1 {
            physical_model_attempts: 1,
            prompt_tokens: u64::MAX,
            completion_tokens: 1,
            total_tokens: u64::MAX,
            reserved_tokens: 0,
            provider_usage_attempts: 1,
            partial_usage_attempts: 0,
            estimated_usage_attempts: 0,
            unknown_usage_attempts: 0,
        };
        assert!(overflowed.validate().is_err());
    }

    #[test]
    fn agent_outcome_evidence_contract_rejects_tampered_or_cancelled_lifecycle() {
        let mut events = lifecycle_events(AgentOutcomeTerminalStatusV1::Completed);
        events[1].metadata.insert(
            AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY.to_string(),
            "9".repeat(64),
        );
        assert!(AgentOutcomeLifecycleBindingV1::from_events(&events).is_err());

        let mut semantic_mismatch = lifecycle_events(AgentOutcomeTerminalStatusV1::Completed);
        semantic_mismatch[0]
            .metadata
            .insert("execution_plan_semantic_sha256".to_string(), "b".repeat(64));
        assert!(AgentOutcomeLifecycleBindingV1::from_events(&semantic_mismatch).is_err());
        assert!(outcome(
            AgentOutcomeTerminalStatusV1::Cancelled,
            0,
            vec![check(true, false, '6')],
        )
        .is_err());
        let mut cancelled = outcome(
            AgentOutcomeTerminalStatusV1::Completed,
            0,
            vec![check(true, false, '6')],
        )
        .unwrap();
        cancelled.lifecycle.terminal_status = AgentOutcomeTerminalStatusV1::Cancelled;
        assert!(cancelled.reward_fraction().is_err());
    }

    #[test]
    fn agent_outcome_evidence_contract_replay_digest_and_json_are_deterministic() {
        let left = outcome(
            AgentOutcomeTerminalStatusV1::Completed,
            0,
            vec![check(false, false, '7'), check(true, false, '6')],
        )
        .unwrap();
        let right = outcome(
            AgentOutcomeTerminalStatusV1::Completed,
            0,
            vec![check(true, false, '6'), check(false, false, '7')],
        )
        .unwrap();
        assert_eq!(left.receipt_sha256, right.receipt_sha256);
        assert_eq!(
            left,
            ExternallyVerifiedOutcomeV1::from_json(&left.to_json().unwrap()).unwrap()
        );
        let mut tampered = left.clone();
        tampered.verifier.subject_sha256 = "8".repeat(64);
        assert!(tampered.validate().is_err());
        assert!(tampered.reward_bps().is_err());
    }
}
