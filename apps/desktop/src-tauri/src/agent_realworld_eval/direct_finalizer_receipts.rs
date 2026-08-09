use super::Treatment;
use crate::agent_finalizer_runtime::direct_finalizer_policy::DIRECT_FINALIZER_RECEIPT_SCHEMA;
use crate::prompt_profile_serving::restore_prompt_profile_selection;
use agent_core::{Event, EventKind, Metadata};
use orchestrator::{
    prompt_genome_sha256, sha256_hex, AgentExecutionMode, AgentRunDecision, ConductorPromptGenome,
    PromptVerification, DIRECT_FINALIZER_PHENOTYPE_SCHEMA,
};
use serde::{Deserialize, Serialize};

const INSTRUCTION_DOMAIN: &str = "cindx.direct-finalizer-instruction.v1\0";
const DELIVERY_ID_DOMAIN: &str = "cindx.direct-finalizer-delivery-id.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DirectFinalizerExecutionReceipt {
    pub(super) prompt_profile_assignment_sha256: String,
    pub(super) phenotype_receipt_sha256: String,
    pub(super) phenotype_sha256: String,
    pub(super) verification: String,
    pub(super) treatment_specific_request_payload_sha256: String,
    pub(super) delivery_request_id_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedReceipt {
    schema: String,
    surface: String,
    verification: String,
    prompt_profile_assignment_sha256: String,
    profile_sha256: String,
    phenotype_sha256: String,
    instruction_sha256: String,
    steer_epoch: u64,
}

pub(super) fn direct_finalizer_receipt_from_events(
    events: &[Event],
    treatment: Treatment,
) -> Result<Option<DirectFinalizerExecutionReceipt>, String> {
    if treatment.is_oracle_reference() {
        return Ok(None);
    }
    let (decision_index, event) = events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, event)| event.summary == "Agent run decision selected")
        .ok_or_else(|| "agent strategy receipt is missing".to_string())?;
    let decision_json = required_metadata_any(&event.metadata, &["run_decision", "decision"])?;
    let decision = serde_json::from_str::<AgentRunDecision>(decision_json)
        .map_err(|error| format!("agent strategy decision receipt is invalid: {error}"))?;
    let genome_json = required_metadata(&event.metadata, "prompt_genome")?;
    let genome = serde_json::from_str::<ConductorPromptGenome>(genome_json)
        .map_err(|error| format!("agent strategy profile receipt is invalid: {error}"))?;
    genome.validate()?;
    let profile_sha256 = prompt_genome_sha256(&genome)?;

    project_direct_finalizer_execution(
        event,
        &events[decision_index.saturating_add(1)..],
        &decision,
        &genome,
        &profile_sha256,
    )
}

pub(super) fn project_direct_finalizer_execution(
    decision_event: &Event,
    following_events: &[Event],
    decision: &AgentRunDecision,
    genome: &ConductorPromptGenome,
    profile_sha256: &str,
) -> Result<Option<DirectFinalizerExecutionReceipt>, String> {
    if decision.execution != AgentExecutionMode::Direct {
        return Ok(None);
    }
    let has_evidence = decision_event
        .metadata
        .contains_key("direct_finalizer_phenotype_schema")
        || following_events.iter().any(|event| {
            [
                "direct_finalizer_phenotype_receipt",
                "direct_finalizer_phenotype_receipt_sha256",
                "direct_finalizer_profile_exercised",
                "direct_finalizer_delivery_request_id",
            ]
            .into_iter()
            .any(|key| event.metadata.contains_key(key))
        });
    if !has_evidence {
        return Ok(None);
    }

    let effort = required_metadata(&decision_event.metadata, "agent_effort")?;
    if !matches!(effort, "auto" | "pro")
        || optional_nonempty(&decision_event.metadata, "collaboration_profile") != Some("direct")
    {
        return Err("direct-finalizer evidence is attached to an ineligible strategy".to_string());
    }
    let scope = optional_nonempty(&decision_event.metadata, "project_id").unwrap_or("global");
    let selection = restore_prompt_profile_selection(effort, scope, &decision_event.metadata)?
        .ok_or_else(|| "direct-finalizer assignment receipt is missing".to_string())?;
    let assignment_sha256 =
        required_sha256(&decision_event.metadata, "prompt_profile_assignment_sha256")?;
    if prompt_genome_sha256(&selection.genome)? != profile_sha256 {
        return Err("direct-finalizer assignment does not match the strategy profile".to_string());
    }

    let phenotype = genome.direct_finalizer_phenotype();
    let phenotype_sha256 = phenotype.sha256()?;
    let verification = verification_label(phenotype.verification);
    if required_metadata(
        &decision_event.metadata,
        "direct_finalizer_phenotype_schema",
    )? != DIRECT_FINALIZER_PHENOTYPE_SCHEMA
        || required_sha256(
            &decision_event.metadata,
            "direct_finalizer_phenotype_sha256",
        )? != phenotype_sha256
        || required_metadata(&decision_event.metadata, "direct_finalizer_verification")?
            != verification
    {
        return Err("direct-finalizer phenotype assignment is invalid".to_string());
    }

    let (terminal_index, terminal) = following_events
        .iter()
        .enumerate()
        .find(|(_, event)| {
            event.kind == EventKind::TaskStatusChanged
                && event.summary == "Agent task completed"
                && optional_nonempty(&event.metadata, "prompt_profile_assignment_sha256")
                    == Some(assignment_sha256.as_str())
        })
        .ok_or_else(|| {
            "direct-finalizer evidence is missing its terminal completion".to_string()
        })?;
    if optional_nonempty(&terminal.metadata, "direct_finalizer_profile_exercised") != Some("true") {
        return Ok(None);
    }
    if optional_nonempty(&terminal.metadata, "finalizer_fallback") != Some("false") {
        return Err(
            "direct-finalizer execution claim is attached to a fallback terminal".to_string(),
        );
    }

    let receipt_json = required_metadata(&terminal.metadata, "direct_finalizer_phenotype_receipt")?;
    let persisted = serde_json::from_str::<PersistedReceipt>(receipt_json)
        .map_err(|error| format!("direct-finalizer phenotype receipt is invalid: {error}"))?;
    let receipt_sha256 = required_sha256(
        &terminal.metadata,
        "direct_finalizer_phenotype_receipt_sha256",
    )?;
    if persisted.schema != DIRECT_FINALIZER_RECEIPT_SCHEMA
        || persisted.surface != "direct_finalizer"
        || persisted.verification != verification
        || persisted.prompt_profile_assignment_sha256 != assignment_sha256
        || persisted.profile_sha256 != profile_sha256
        || persisted.phenotype_sha256 != phenotype_sha256
        || persisted.instruction_sha256
            != domain_hash(
                INSTRUCTION_DOMAIN,
                phenotype.directive().unwrap_or_default(),
            )
        || receipt_sha256
            != domain_hash(
                &format!("{DIRECT_FINALIZER_RECEIPT_SCHEMA}\0"),
                receipt_json,
            )
        || required_sha256(&terminal.metadata, "direct_finalizer_phenotype_sha256")?
            != phenotype_sha256
        || optional_nonempty(&terminal.metadata, "direct_finalizer_verification")
            != Some(verification)
        || metadata_epoch(terminal) != Some(persisted.steer_epoch)
    {
        return Err("direct-finalizer phenotype receipt identity is invalid".to_string());
    }

    let delivery_request_id =
        required_metadata(&terminal.metadata, "direct_finalizer_delivery_request_id")?;
    let matches_receipt = |event: &Event| {
        optional_nonempty(&event.metadata, "direct_finalizer_phenotype_receipt_sha256")
            == Some(receipt_sha256.as_str())
            && optional_nonempty(&event.metadata, "prompt_profile_assignment_sha256")
                == Some(assignment_sha256.as_str())
            && metadata_epoch(event) == Some(persisted.steer_epoch)
    };
    let (started_index, started) = following_events[..terminal_index]
        .iter()
        .enumerate()
        .find(|(_, event)| {
            event.kind == EventKind::ModelRequestStarted
                && event.summary == "Agent model turn started"
                && optional_nonempty(&event.metadata, "request_id") == Some(delivery_request_id)
                && matches_receipt(event)
        })
        .ok_or_else(|| "matching direct-finalizer request event is missing".to_string())?;
    if optional_nonempty(&started.metadata, "execution_role") != Some("finalizer")
        || optional_nonempty(&started.metadata, "terminal_commit") != Some("true")
    {
        return Err("matching model request was not a terminal Finalizer turn".to_string());
    }
    let finished = following_events[started_index.saturating_add(1)..terminal_index]
        .iter()
        .find(|event| {
            event.kind == EventKind::ModelRequestFinished
                && event.summary == "Agent model turn finished"
                && optional_nonempty(&event.metadata, "request_id") == Some(delivery_request_id)
                && matches_receipt(event)
        })
        .ok_or_else(|| "matching direct-finalizer finished event is missing".to_string())?;

    Ok(Some(DirectFinalizerExecutionReceipt {
        prompt_profile_assignment_sha256: assignment_sha256,
        phenotype_receipt_sha256: receipt_sha256,
        phenotype_sha256,
        verification: verification.to_string(),
        treatment_specific_request_payload_sha256: required_sha256(
            &finished.metadata,
            "request_payload_sha256",
        )?,
        delivery_request_id_sha256: domain_hash(DELIVERY_ID_DOMAIN, delivery_request_id),
    }))
}

fn metadata_epoch(event: &Event) -> Option<u64> {
    event.metadata.get("steer_epoch")?.parse().ok()
}

fn verification_label(value: PromptVerification) -> &'static str {
    match value {
        PromptVerification::Minimal => "minimal",
        PromptVerification::Evidence => "evidence",
        PromptVerification::Adversarial => "adversarial",
    }
}

fn required_metadata<'a>(metadata: &'a Metadata, key: &str) -> Result<&'a str, String> {
    optional_nonempty(metadata, key)
        .ok_or_else(|| format!("required receipt field {key} is missing"))
}

fn required_metadata_any<'a>(metadata: &'a Metadata, keys: &[&str]) -> Result<&'a str, String> {
    keys.iter()
        .find_map(|key| optional_nonempty(metadata, key))
        .ok_or_else(|| format!("required receipt field {} is missing", keys.join(" or ")))
}

fn optional_nonempty<'a>(metadata: &'a Metadata, key: &str) -> Option<&'a str> {
    metadata
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_sha256(metadata: &Metadata, key: &str) -> Result<String, String> {
    let value = required_metadata(metadata, key)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!("required digest receipt field {key} is invalid"));
    }
    Ok(value.to_string())
}

fn domain_hash(domain: &str, value: &str) -> String {
    sha256_hex(format!("{domain}{value}").as_bytes())
}
