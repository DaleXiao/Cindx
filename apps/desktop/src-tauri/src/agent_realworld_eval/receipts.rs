use super::{metadata_u64, Treatment};
use crate::*;
use agent_core::Event;

const PROVIDER_RESPONSE_ID_DOMAIN: &str = "cindx.provider-response-id.v1\0";
const PROVIDER_FINGERPRINT_DOMAIN: &str = "cindx.provider-system-fingerprint.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ResolvedBudgetReceipt {
    pub(super) max_duration_ms: u64,
    pub(super) model_call_timeout_ms: u64,
    pub(super) tool_call_timeout_ms: u64,
    pub(super) initial_model_calls: usize,
    pub(super) max_model_calls: usize,
    pub(super) initial_tool_calls: usize,
    pub(super) max_tool_calls: usize,
    pub(super) no_progress_timeout_ms: u64,
    pub(super) max_total_tokens: u64,
    pub(super) max_physical_model_attempts: usize,
    pub(super) terminal_token_reserve: u64,
    pub(super) terminal_physical_model_attempt_reserve: usize,
}

impl ResolvedBudgetReceipt {
    pub(super) fn for_treatment(treatment: Treatment) -> Self {
        let effort = treatment.product_effort().unwrap_or("fast");
        Self::from_budget(RunBudget::for_effort(effort))
    }

    fn from_budget(budget: RunBudget) -> Self {
        Self {
            max_duration_ms: duration_ms(budget.max_duration),
            model_call_timeout_ms: duration_ms(budget.model_call_timeout),
            tool_call_timeout_ms: duration_ms(budget.tool_call_timeout),
            initial_model_calls: budget.initial_model_calls,
            max_model_calls: budget.max_model_calls,
            initial_tool_calls: budget.initial_tool_calls,
            max_tool_calls: budget.max_tool_calls,
            no_progress_timeout_ms: duration_ms(budget.no_progress_timeout),
            max_total_tokens: budget.max_total_tokens,
            max_physical_model_attempts: budget.max_physical_model_attempts,
            terminal_token_reserve: budget.terminal_token_reserve,
            terminal_physical_model_attempt_reserve: budget.terminal_physical_model_attempt_reserve,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct StrategyReceipt {
    pub(super) requested_policy: String,
    pub(super) effective_policy: String,
    pub(super) execution_mode: String,
    pub(super) decision_source: String,
    pub(super) execution_constraint: String,
    pub(super) decision_sha256: String,
    pub(super) routing_signature_sha256: String,
    pub(super) profile_source: String,
    pub(super) profile_id: String,
    pub(super) profile_sha256: String,
    pub(super) profile_generation: u32,
    pub(super) parent_profile_ids: Vec<String>,
    pub(super) learned_artifact_sha256: Option<String>,
    pub(super) learned_method: Option<String>,
    pub(super) stable_profile_id: Option<String>,
    pub(super) dataset_sha256: Option<String>,
    pub(super) paired_evidence_sha256: Option<String>,
    pub(super) promotion_gate_protocol: Option<String>,
    pub(super) workflow_profile_exercised: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ModelReceipt {
    pub(super) configured_model: String,
    pub(super) request_payload_sha256: String,
    pub(super) provider_response_model: Option<String>,
    pub(super) provider_response_id_sha256: Option<String>,
    pub(super) provider_system_fingerprint_sha256: Option<String>,
    pub(super) receipt_status: String,
    pub(super) response_semantic_sha256: String,
}

pub(super) fn resolved_budget_from_events(
    events: &[Event],
    treatment: Treatment,
) -> Result<ResolvedBudgetReceipt, String> {
    let expected = ResolvedBudgetReceipt::for_treatment(treatment);
    if treatment.is_oracle_reference() {
        return Ok(expected);
    }
    let event = events
        .iter()
        .find(|event| event.summary == "Agent task started")
        .ok_or_else(|| "agent task start budget receipt is missing".to_string())?;
    let observed = ResolvedBudgetReceipt {
        max_duration_ms: required_u64(&event.metadata, "run_budget_ms")?,
        model_call_timeout_ms: required_u64(&event.metadata, "run_model_timeout_ms")?,
        tool_call_timeout_ms: required_u64(&event.metadata, "run_tool_timeout_ms")?,
        initial_model_calls: required_usize(&event.metadata, "run_initial_model_calls")?,
        max_model_calls: required_usize(&event.metadata, "run_model_call_budget")?,
        initial_tool_calls: required_usize(&event.metadata, "run_initial_tool_calls")?,
        max_tool_calls: required_usize(&event.metadata, "run_tool_call_budget")?,
        no_progress_timeout_ms: required_u64(&event.metadata, "run_no_progress_ms")?,
        max_total_tokens: required_u64(&event.metadata, "run_total_token_budget")?,
        max_physical_model_attempts: required_usize(
            &event.metadata,
            "run_physical_model_attempt_budget",
        )?,
        terminal_token_reserve: required_u64(&event.metadata, "run_terminal_token_reserve")?,
        terminal_physical_model_attempt_reserve: required_usize(
            &event.metadata,
            "run_terminal_physical_model_attempt_reserve",
        )?,
    };
    if observed != expected {
        return Err(
            "persisted run budget does not match the resolved treatment budget".to_string(),
        );
    }
    Ok(observed)
}

pub(super) fn strategy_receipt_from_events(
    events: &[Event],
    treatment: Treatment,
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
) -> Result<Option<StrategyReceipt>, String> {
    if treatment.is_oracle_reference() {
        return Ok(None);
    }
    let event = events
        .iter()
        .rev()
        .find(|event| event.summary == "Agent run decision selected")
        .ok_or_else(|| "agent strategy receipt is missing".to_string())?;
    let decision_json = required_metadata_any(&event.metadata, &["run_decision", "decision"])?;
    let decision = serde_json::from_str::<AgentRunDecision>(decision_json)
        .map_err(|error| format!("agent strategy decision receipt is invalid: {error}"))?;
    let execution_constraint = event
        .metadata
        .get("execution_constraint")
        .map(String::as_str)
        .unwrap_or("native");
    if treatment.is_memory_evaluation() {
        if execution_constraint != "matched_memory_effect"
            || decision.execution != orchestrator::AgentExecutionMode::Direct
        {
            return Err(
                "memory-effect receipt must prove its matched direct constraint".to_string(),
            );
        }
    } else if treatment.is_grounded_direct() {
        if execution_constraint != "grounded_direct"
            || decision.execution != orchestrator::AgentExecutionMode::Direct
        {
            return Err(
                "grounded-direct receipt must prove its direct execution constraint".to_string(),
            );
        }
    } else if execution_constraint != "native" {
        return Err("native treatment claimed an evaluation execution constraint".to_string());
    }
    let genome_json = required_metadata(&event.metadata, "prompt_genome")?;
    let genome = serde_json::from_str::<ConductorPromptGenome>(genome_json)
        .map_err(|error| format!("agent strategy profile receipt is invalid: {error}"))?;
    genome.validate()?;
    let profile_sha256 = prompt_genome_sha256(&genome)?;
    let profile_source = required_metadata_any(
        &event.metadata,
        &["profile_source", "prompt_profile_source"],
    )?
    .to_string();
    let workflow_profile_exercised =
        if decision.execution == orchestrator::AgentExecutionMode::Workflow {
            events.iter().any(|event| {
                event.summary == "Conductor prompt profile selected"
                    && event.metadata.get("prompt_profile") == Some(&genome.id)
            })
        } else {
            false
        };

    let learned = match (frozen_profile, profile_source.as_str()) {
        (Some(snapshot), "evaluation_frozen_profile") => {
            snapshot.validate()?;
            if snapshot.effort != treatment.profile_effort().unwrap_or_default()
                || snapshot.genome.id != genome.id
                || snapshot.candidate_sha256 != profile_sha256
            {
                return Err(
                    "frozen profile receipt does not match the exercised strategy".to_string(),
                );
            }
            Some(snapshot)
        }
        (Some(_), _) => {
            return Err("frozen evaluation profile was configured but not exercised".to_string())
        }
        (None, "evaluation_frozen_profile") => {
            return Err(
                "strategy claims a frozen evaluation profile without an artifact".to_string(),
            )
        }
        (None, _) => None,
    };
    let learned_method = learned
        .map(|snapshot| serde_json::to_value(snapshot.evolution_method))
        .transpose()
        .map_err(|error| format!("failed to serialize learned profile method: {error}"))?
        .and_then(|value| value.as_str().map(str::to_string));
    let routing_signature = required_metadata(&event.metadata, "routing_signature")?;
    Ok(Some(StrategyReceipt {
        requested_policy: required_metadata(&event.metadata, "requested_policy")?.to_string(),
        effective_policy: required_metadata(&event.metadata, "collaboration_policy")?.to_string(),
        execution_mode: match decision.execution {
            orchestrator::AgentExecutionMode::Direct => "direct",
            orchestrator::AgentExecutionMode::Workflow => "workflow",
        }
        .to_string(),
        decision_source: required_metadata(&event.metadata, "decision_source")?.to_string(),
        execution_constraint: execution_constraint.to_string(),
        decision_sha256: domain_hash("cindx.agent-run-decision.v1\0", decision_json),
        routing_signature_sha256: domain_hash(
            "cindx.agent-routing-signature.v1\0",
            routing_signature,
        ),
        profile_source,
        profile_id: genome.id.clone(),
        profile_sha256,
        profile_generation: genome.generation,
        parent_profile_ids: genome.parents.clone(),
        learned_artifact_sha256: learned
            .map(FrozenPromptProfileSnapshot::artifact_sha256)
            .transpose()?,
        learned_method,
        stable_profile_id: learned.map(|snapshot| snapshot.stable_profile_id.clone()),
        dataset_sha256: learned.map(|snapshot| snapshot.dataset_sha256.clone()),
        paired_evidence_sha256: learned.map(|snapshot| snapshot.paired_evidence_sha256.clone()),
        promotion_gate_protocol: learned.map(|snapshot| snapshot.promotion_gate_protocol.clone()),
        workflow_profile_exercised,
    }))
}

pub(super) fn model_receipts_from_metadata(
    metadata: &Metadata,
) -> Result<Vec<ModelReceipt>, String> {
    if let Some(encoded) = metadata.get("worker_provider_receipts") {
        let receipts = serde_json::from_str::<Vec<Metadata>>(encoded)
            .map_err(|error| format!("worker provider receipts are invalid: {error}"))?;
        if receipts.is_empty() {
            return Err("worker provider receipt list is empty".to_string());
        }
        return receipts.iter().map(model_receipt_from_metadata).collect();
    }
    Ok(vec![model_receipt_from_metadata(metadata)?])
}

fn model_receipt_from_metadata(metadata: &Metadata) -> Result<ModelReceipt, String> {
    let request_payload_sha256 = required_sha256(metadata, "request_payload_sha256")?;
    let response_semantic_sha256 = required_sha256(metadata, "response_semantic_sha256")?;
    let response_id = optional_nonempty(metadata, "provider_response_id");
    let receipt_status = required_metadata(metadata, "provider_receipt_status")?.to_string();
    match (receipt_status.as_str(), response_id) {
        ("observed", Some(_)) | ("identity_conflict", _) => {}
        ("provider_id_missing", None) => {}
        _ => {
            return Err(
                "provider receipt status is inconsistent with the provider response id".to_string(),
            )
        }
    }
    let configured_model = optional_nonempty(metadata, "model")
        .or_else(|| optional_nonempty(metadata, "agent_model"))
        .ok_or_else(|| "provider receipt configured model is missing".to_string())?;
    Ok(ModelReceipt {
        configured_model: configured_model.to_string(),
        request_payload_sha256,
        provider_response_model: optional_nonempty(metadata, "provider_response_model")
            .map(str::to_string),
        provider_response_id_sha256: response_id
            .map(|value| domain_hash(PROVIDER_RESPONSE_ID_DOMAIN, value)),
        provider_system_fingerprint_sha256: optional_nonempty(
            metadata,
            "provider_system_fingerprint",
        )
        .map(|value| domain_hash(PROVIDER_FINGERPRINT_DOMAIN, value)),
        receipt_status,
        response_semantic_sha256,
    })
}

pub(super) fn successful_response_count(event: &Event) -> usize {
    if event.kind != EventKind::ModelRequestFinished {
        return 0;
    }
    if let Some(responses) = event
        .metadata
        .get("worker_model_responses")
        .and_then(|value| value.parse::<usize>().ok())
    {
        return responses;
    }
    if event.metadata.contains_key("request_payload_sha256") {
        return 1;
    }
    usize::from(
        event.summary == "Agent model turn finished"
            || event.metadata.get("status").map(String::as_str) == Some("completed")
            || event.metadata.contains_key("output"),
    )
}

pub(super) fn is_receipt_bearing_event(event: &Event) -> bool {
    event.kind == EventKind::ModelRequestFinished
        && (event.metadata.contains_key("request_payload_sha256")
            || event.metadata.contains_key("worker_provider_receipts"))
}

fn required_metadata<'a>(metadata: &'a Metadata, key: &str) -> Result<&'a str, String> {
    optional_nonempty(metadata, key)
        .ok_or_else(|| format!("required receipt field {key} is missing"))
}

fn required_metadata_any<'a>(metadata: &'a Metadata, keys: &[&str]) -> Result<&'a str, String> {
    keys.iter()
        .find_map(|key| optional_nonempty(metadata, key))
        .ok_or_else(|| format!("required receipt field {} is missing", keys.join("/")))
}

fn optional_nonempty<'a>(metadata: &'a Metadata, key: &str) -> Option<&'a str> {
    metadata
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_u64(metadata: &Metadata, key: &str) -> Result<u64, String> {
    let value = metadata_u64(metadata, key);
    if value == 0 && metadata.get(key).map(String::as_str) != Some("0") {
        return Err(format!("required numeric receipt field {key} is invalid"));
    }
    Ok(value)
}

fn required_usize(metadata: &Metadata, key: &str) -> Result<usize, String> {
    required_metadata(metadata, key)?
        .parse::<usize>()
        .map_err(|_| format!("required numeric receipt field {key} is invalid"))
}

fn required_sha256(metadata: &Metadata, key: &str) -> Result<String, String> {
    let value = required_metadata(metadata, key)?;
    if !is_sha256(value) {
        return Err(format!("required digest receipt field {key} is invalid"));
    }
    Ok(value.to_string())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn domain_hash(domain: &str, value: &str) -> String {
    sha256_hex(format!("{domain}{value}").as_bytes())
}

fn duration_ms(value: std::time::Duration) -> u64 {
    value.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, TaskId};

    fn event(summary: &str, kind: EventKind, metadata: Metadata) -> Event {
        Event {
            id: EventId("event-receipt".to_string()),
            task_id: TaskId("task-receipt".to_string()),
            sequence: 7,
            timestamp_ms: 1,
            kind,
            summary: summary.to_string(),
            metadata,
        }
    }

    #[test]
    fn provider_receipt_hashes_upstream_identity_without_serializing_the_raw_id() {
        let raw_id = "upstream-response-private-123";
        let receipt = model_receipts_from_metadata(
            &[
                ("model".to_string(), "configured-model".to_string()),
                ("request_payload_sha256".to_string(), "a".repeat(64)),
                ("provider_response_id".to_string(), raw_id.to_string()),
                (
                    "provider_response_model".to_string(),
                    "served-model".to_string(),
                ),
                ("response_semantic_sha256".to_string(), "b".repeat(64)),
                (
                    "provider_receipt_status".to_string(),
                    "observed".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        )
        .expect("receipt should validate")
        .remove(0);
        let encoded = serde_json::to_string(&receipt).expect("receipt should serialize");

        assert_eq!(receipt.configured_model, "configured-model");
        assert_eq!(
            receipt.provider_response_model.as_deref(),
            Some("served-model")
        );
        assert!(receipt
            .provider_response_id_sha256
            .as_deref()
            .is_some_and(is_sha256));
        assert!(!encoded.contains(raw_id));
    }

    #[test]
    fn missing_and_conflicting_provider_ids_remain_explicit() {
        for status in ["provider_id_missing", "identity_conflict"] {
            let metadata = [
                ("model".to_string(), "configured-model".to_string()),
                ("request_payload_sha256".to_string(), "a".repeat(64)),
                ("response_semantic_sha256".to_string(), "b".repeat(64)),
                ("provider_receipt_status".to_string(), status.to_string()),
            ]
            .into_iter()
            .collect::<Metadata>();
            let receipt = model_receipts_from_metadata(&metadata)
                .expect("explicit incomplete receipt should parse")
                .remove(0);
            assert_eq!(receipt.receipt_status, status);
            assert!(receipt.provider_response_id_sha256.is_none());
        }
    }

    #[test]
    fn degraded_collaboration_with_provider_receipt_counts_as_a_response() {
        let event = event(
            "Collaboration verifier unavailable",
            EventKind::ModelRequestFinished,
            Metadata::from([
                ("status".to_string(), "degraded".to_string()),
                ("request_payload_sha256".to_string(), "a".repeat(64)),
                ("response_semantic_sha256".to_string(), "b".repeat(64)),
                (
                    "provider_receipt_status".to_string(),
                    "observed".to_string(),
                ),
            ]),
        );

        assert_eq!(successful_response_count(&event), 1);
    }

    #[test]
    fn degraded_collaboration_without_provider_receipt_does_not_count_as_a_response() {
        let event = event(
            "Collaboration verifier unavailable",
            EventKind::ModelRequestFinished,
            Metadata::from([("status".to_string(), "degraded".to_string())]),
        );

        assert_eq!(successful_response_count(&event), 0);
    }

    #[test]
    fn completed_event_without_provider_receipt_still_exposes_missing_evidence() {
        let event = event(
            "Collaboration verifier finished",
            EventKind::ModelRequestFinished,
            Metadata::from([("status".to_string(), "completed".to_string())]),
        );

        assert_eq!(successful_response_count(&event), 1);
    }

    #[test]
    fn persisted_product_budget_must_match_the_resolved_treatment_budget() {
        let budget = ResolvedBudgetReceipt::for_treatment(Treatment::Auto);
        assert_eq!(
            ResolvedBudgetReceipt::for_treatment(Treatment::GroundedDirect),
            budget,
            "grounded direct must remain iso-budget with Auto"
        );
        let mut metadata = Metadata::from([
            (
                "run_budget_ms".to_string(),
                budget.max_duration_ms.to_string(),
            ),
            (
                "run_model_timeout_ms".to_string(),
                budget.model_call_timeout_ms.to_string(),
            ),
            (
                "run_tool_timeout_ms".to_string(),
                budget.tool_call_timeout_ms.to_string(),
            ),
            (
                "run_initial_model_calls".to_string(),
                budget.initial_model_calls.to_string(),
            ),
            (
                "run_model_call_budget".to_string(),
                budget.max_model_calls.to_string(),
            ),
            (
                "run_initial_tool_calls".to_string(),
                budget.initial_tool_calls.to_string(),
            ),
            (
                "run_tool_call_budget".to_string(),
                budget.max_tool_calls.to_string(),
            ),
            (
                "run_no_progress_ms".to_string(),
                budget.no_progress_timeout_ms.to_string(),
            ),
            (
                "run_total_token_budget".to_string(),
                budget.max_total_tokens.to_string(),
            ),
            (
                "run_physical_model_attempt_budget".to_string(),
                budget.max_physical_model_attempts.to_string(),
            ),
            (
                "run_terminal_token_reserve".to_string(),
                budget.terminal_token_reserve.to_string(),
            ),
            (
                "run_terminal_physical_model_attempt_reserve".to_string(),
                budget.terminal_physical_model_attempt_reserve.to_string(),
            ),
        ]);
        let events = vec![event(
            "Agent task started",
            EventKind::TaskStatusChanged,
            metadata.clone(),
        )];
        assert_eq!(
            resolved_budget_from_events(&events, Treatment::Auto).expect("budget receipt"),
            budget
        );

        metadata.insert("run_model_call_budget".to_string(), "1".to_string());
        let drifted = vec![event(
            "Agent task started",
            EventKind::TaskStatusChanged,
            metadata,
        )];
        assert!(resolved_budget_from_events(&drifted, Treatment::Auto).is_err());
    }

    #[test]
    fn strategy_receipt_binds_the_actual_decision_and_profile() {
        let decision = AgentRunDecision::direct("configured-model");
        let genome = ConductorPromptGenome::seed_for_effort("fast");
        let metadata = Metadata::from([
            (
                "run_decision".to_string(),
                serde_json::to_string(&decision).unwrap(),
            ),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).unwrap(),
            ),
            ("profile_source".to_string(), "seed_fallback".to_string()),
            ("requested_policy".to_string(), "single".to_string()),
            ("collaboration_policy".to_string(), "single".to_string()),
            ("decision_source".to_string(), "fast_direct".to_string()),
            ("routing_signature".to_string(), "frozen-route".to_string()),
        ]);
        let receipt = strategy_receipt_from_events(
            &[event(
                "Agent run decision selected",
                EventKind::TaskStatusChanged,
                metadata,
            )],
            Treatment::Fast,
            None,
        )
        .expect("strategy receipt")
        .expect("product strategy");

        assert_eq!(receipt.profile_id, genome.id);
        assert_eq!(
            receipt.profile_sha256,
            prompt_genome_sha256(&genome).unwrap()
        );
        assert_eq!(receipt.execution_mode, "direct");
        assert!(receipt.learned_artifact_sha256.is_none());
    }

    #[test]
    fn grounded_direct_receipt_proves_the_execution_constraint() {
        let decision = AgentRunDecision::direct("configured-model");
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let metadata = Metadata::from([
            (
                "run_decision".to_string(),
                serde_json::to_string(&decision).unwrap(),
            ),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).unwrap(),
            ),
            ("profile_source".to_string(), "seed_fallback".to_string()),
            ("requested_policy".to_string(), "auto_router".to_string()),
            ("collaboration_policy".to_string(), "single".to_string()),
            ("decision_source".to_string(), "fixture".to_string()),
            ("routing_signature".to_string(), "frozen-route".to_string()),
            (
                "execution_constraint".to_string(),
                "grounded_direct".to_string(),
            ),
        ]);
        let receipt = strategy_receipt_from_events(
            &[event(
                "Agent run decision selected",
                EventKind::TaskStatusChanged,
                metadata,
            )],
            Treatment::GroundedDirect,
            None,
        )
        .expect("strategy receipt")
        .expect("grounded strategy");

        assert_eq!(receipt.execution_constraint, "grounded_direct");
        assert_eq!(receipt.execution_mode, "direct");
    }

    #[test]
    fn memory_effect_receipt_proves_the_matched_constraint() {
        let mut decision = AgentRunDecision::direct("configured-model");
        decision.memory = orchestrator::MemoryRecallPlan {
            policy: orchestrator::MemoryRecallPolicy::Relevant,
            query: "durable project requirements and prior-session facts".to_string(),
        };
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let metadata = Metadata::from([
            (
                "run_decision".to_string(),
                serde_json::to_string(&decision).unwrap(),
            ),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).unwrap(),
            ),
            ("profile_source".to_string(), "seed_fallback".to_string()),
            ("requested_policy".to_string(), "auto_router".to_string()),
            ("collaboration_policy".to_string(), "single".to_string()),
            (
                "decision_source".to_string(),
                "matched_memory_evaluation".to_string(),
            ),
            ("routing_signature".to_string(), "frozen-route".to_string()),
            (
                "execution_constraint".to_string(),
                "matched_memory_effect".to_string(),
            ),
        ]);

        let receipt = strategy_receipt_from_events(
            &[event(
                "Agent run decision selected",
                EventKind::TaskStatusChanged,
                metadata,
            )],
            Treatment::MemoryOn,
            None,
        )
        .expect("strategy receipt")
        .expect("memory-effect strategy");

        assert_eq!(receipt.decision_source, "matched_memory_evaluation");
        assert_eq!(receipt.execution_constraint, "matched_memory_effect");
        assert_eq!(receipt.execution_mode, "direct");
    }
}
