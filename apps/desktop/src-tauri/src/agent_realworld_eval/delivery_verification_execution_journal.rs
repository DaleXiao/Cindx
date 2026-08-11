use super::delivery_verification::DeliveryVerificationFindingCounts;
use super::delivery_verification_authorization::*;
use super::delivery_verification_protocol::DELIVERY_VERIFICATION_EXECUTION_JOURNAL_SCHEMA;
use agent_core::ModelRole;
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[path = "delivery_verification_execution_storage.rs"]
mod storage;
use self::storage::*;

pub(super) const DELIVERY_EXECUTION_JOURNAL_SCHEMA: &str =
    DELIVERY_VERIFICATION_EXECUTION_JOURNAL_SCHEMA;
pub(super) const DELIVERY_EXECUTION_RECOVERY_SCHEMA: &str =
    "cindx.agent-eval.delivery-verification-execution-recovery.v3";
pub(super) const DELIVERY_EXECUTION_JOURNAL_FILE_NAME: &str =
    "delivery-verification-execution-journal.json";
pub(super) const DELIVERY_EXECUTION_RECOVERY_FILE_NAME: &str =
    "delivery-verification-execution-recovery.json";
pub(super) const DELIVERY_EXECUTION_LOCK_FILE_NAME: &str = "delivery-verification-execution.lock";

const JOURNAL_HASH_DOMAIN: &[u8] = b"cindx.agent-eval.delivery-verification-execution-journal.v3\0";
const CAMPAIGN_RESERVATION_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-campaign-reservation.v3\0";
const CALL_RESERVATION_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-call-reservation.v3\0";
const CALL_TERMINAL_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-call-receipt.v3\0";
const CASE_TERMINAL_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-case-receipt.v3\0";
const DECISION_HASH_DOMAIN: &[u8] = b"cindx.agent-eval.delivery-verification-decision-receipt.v3\0";
const TERMINAL_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-campaign-terminal.v3\0";
const RECOVERY_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-execution-recovery.v3\0";
const MAX_SEMANTIC_REQUEST_BYTES: u64 = 512 * 1024;
const MAX_WIRE_PAYLOAD_BYTES: u64 = 512 * 1024;
const CASE_COUNT: usize = 32;
const CALIBRATION_CASES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeliveryVerificationCallStageV1 {
    VerifierInitial,
    OwnerRepair,
    VerifierRecheck,
}

const CALL_STAGES: [DeliveryVerificationCallStageV1; 3] = [
    DeliveryVerificationCallStageV1::VerifierInitial,
    DeliveryVerificationCallStageV1::OwnerRepair,
    DeliveryVerificationCallStageV1::VerifierRecheck,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeliveryVerificationCallTerminalStatusV1 {
    Completed,
    ProviderFailure,
    InvalidOutput,
    Cancelled,
    Steered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeliveryVerificationCaseTerminalStatusV1 {
    Complete,
    StructuralFailure,
    TreatmentExecutionFailure,
    Censored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeliveryVerificationCampaignDispositionV1 {
    SeededRepairEffective,
    NotEffective,
    PreservationRegression,
    TerminalFutility,
    Inconclusive,
    Invalid,
    Censored,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationChargedResourcesV1 {
    pub(super) logical_model_calls: u64,
    pub(super) physical_model_attempts: u64,
    pub(super) reserved_output_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationObservedResourcesV1 {
    pub(super) terminal_model_calls: u64,
    pub(super) latency_ms: u64,
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationCallUsageV1 {
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) usage_source: String,
    pub(super) usage_estimated: bool,
}

#[derive(Debug, Clone)]
pub(super) struct DeliveryVerificationCallReservationInputV1 {
    pub(super) case_ordinal: usize,
    pub(super) stage: DeliveryVerificationCallStageV1,
    pub(super) role: ModelRole,
    pub(super) configured_model_sha256: String,
    pub(super) semantic_request_sha256: String,
    pub(super) semantic_request_bytes: u64,
    pub(super) wire_payload_sha256: String,
    pub(super) wire_payload_bytes: u64,
    pub(super) max_output_tokens: u64,
    pub(super) reserved_at_ms: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct DeliveryVerificationCallPermitV1 {
    case_ordinal: usize,
    global_call_ordinal: usize,
    stage: DeliveryVerificationCallStageV1,
    reservation_sha256: String,
}

impl DeliveryVerificationCallPermitV1 {
    #[cfg(test)]
    pub(super) fn global_call_ordinal(&self) -> usize {
        self.global_call_ordinal
    }
}

#[derive(Debug)]
pub(super) struct DeliveryVerificationCallTerminalInputV1 {
    pub(super) permit: DeliveryVerificationCallPermitV1,
    pub(super) status: DeliveryVerificationCallTerminalStatusV1,
    pub(super) failure_class: Option<String>,
    pub(super) retryable: Option<bool>,
    pub(super) provider_status_code: Option<u16>,
    pub(super) latency_ms: u64,
    pub(super) request_payload_sha256: Option<String>,
    pub(super) response_semantic_sha256: Option<String>,
    pub(super) provider_response_id_sha256: Option<String>,
    pub(super) provider_response_model_sha256: Option<String>,
    pub(super) provider_system_fingerprint_sha256: Option<String>,
    pub(super) provider_receipt_status: Option<String>,
    pub(super) usage: Option<DeliveryVerificationCallUsageV1>,
    pub(super) terminal_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryVerificationResponseArtifactV1 {
    pub(super) sha256: String,
    pub(super) bytes: u64,
    pub(super) path: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct DeliveryVerificationCaseTerminalInputV1 {
    pub(super) case_ordinal: usize,
    pub(super) status: DeliveryVerificationCaseTerminalStatusV1,
    pub(super) control_passed: Option<bool>,
    pub(super) treatment_passed: Option<bool>,
    pub(super) seeded_candidate_sha256: Option<String>,
    pub(super) seeded_candidate_bytes: Option<u64>,
    pub(super) control_output_sha256: Option<String>,
    pub(super) treatment_output_sha256: Option<String>,
    pub(super) initial_verifier_decision: Option<String>,
    pub(super) initial_finding_counts: DeliveryVerificationFindingCounts,
    pub(super) repair_activated: bool,
    pub(super) recheck_decision: Option<String>,
    pub(super) recheck_finding_counts: DeliveryVerificationFindingCounts,
    pub(super) treatment_disposition: Option<String>,
    pub(super) failure_stage: Option<String>,
    pub(super) failure_code: Option<String>,
    pub(super) outcome_reason: String,
    pub(super) observation_sha256: String,
    pub(super) completed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeliveryVerificationRecoveryV1 {
    Terminal(DeliveryVerificationCampaignDispositionV1),
    RecoveryTerminal(DeliveryVerificationCampaignDispositionV1),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalPhaseV1 {
    AuthorizationConsumed,
    CampaignReserved,
    Executing,
    Terminal,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum JournalCaseStateV1 {
    Planned,
    Reserved { reserved_at_ms: u64 },
    Terminal { receipt: CaseTerminalReceiptV1 },
    Skipped { reason: String, closed_at_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum JournalCallStateV1 {
    Planned,
    Reserved {
        reservation: CallReservationReceiptV1,
    },
    Terminal {
        reservation: CallReservationReceiptV1,
        receipt: Box<CallTerminalReceiptV1>,
    },
    NotRequired {
        reason: String,
        closed_at_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalCallV1 {
    stage: DeliveryVerificationCallStageV1,
    state: JournalCallStateV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalCaseV1 {
    binding: DeliveryVerificationAuthorizationCaseV1,
    state: JournalCaseStateV1,
    calls: Vec<JournalCallV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CampaignReservationReceiptV1 {
    max_logical_model_calls: usize,
    max_physical_model_attempts: usize,
    max_total_tokens: u64,
    max_duration_ms: u64,
    reserved_at_ms: u64,
    reservation_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallReservationReceiptV1 {
    case_ordinal: usize,
    global_call_ordinal: usize,
    stage: DeliveryVerificationCallStageV1,
    role: String,
    configured_model_sha256: String,
    semantic_request_sha256: String,
    semantic_request_bytes: u64,
    wire_payload_sha256: String,
    wire_payload_bytes: u64,
    max_output_tokens: u64,
    reserved_at_ms: u64,
    reservation_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallTerminalReceiptV1 {
    status: DeliveryVerificationCallTerminalStatusV1,
    failure_class: Option<String>,
    retryable: Option<bool>,
    provider_status_code: Option<u16>,
    latency_ms: u64,
    response_artifact_sha256: Option<String>,
    response_artifact_bytes: Option<u64>,
    request_payload_sha256: Option<String>,
    response_semantic_sha256: Option<String>,
    provider_response_id_sha256: Option<String>,
    provider_response_model_sha256: Option<String>,
    provider_system_fingerprint_sha256: Option<String>,
    provider_receipt_status: Option<String>,
    usage: Option<DeliveryVerificationCallUsageV1>,
    terminal_at_ms: u64,
    terminal_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseTerminalReceiptV1 {
    status: DeliveryVerificationCaseTerminalStatusV1,
    control_passed: Option<bool>,
    treatment_passed: Option<bool>,
    seeded_candidate_sha256: Option<String>,
    seeded_candidate_bytes: Option<u64>,
    control_output_sha256: Option<String>,
    treatment_output_sha256: Option<String>,
    initial_verifier_decision: Option<String>,
    initial_finding_counts: DeliveryVerificationFindingCounts,
    repair_activated: bool,
    recheck_decision: Option<String>,
    recheck_finding_counts: DeliveryVerificationFindingCounts,
    treatment_disposition: Option<String>,
    failure_stage: Option<String>,
    failure_code: Option<String>,
    outcome_reason: String,
    observation_sha256: String,
    resources: DeliveryVerificationObservedResourcesV1,
    completed_at_ms: u64,
    receipt_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionReceiptV1 {
    stage: String,
    decision: String,
    counts_sha256: String,
    decided_at_ms: u64,
    decision_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CampaignTerminalReceiptV1 {
    disposition: DeliveryVerificationCampaignDispositionV1,
    reason: String,
    evidence_sha256: String,
    terminal_at_ms: u64,
    terminal_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalDocumentV1 {
    schema: String,
    revision: u64,
    authorization: DeliveryVerificationAuthorizationV1,
    tombstone_sha256: String,
    authorization_consumed_at_ms: u64,
    phase: JournalPhaseV1,
    campaign_reservation: Option<CampaignReservationReceiptV1>,
    cases: Vec<JournalCaseV1>,
    charged: DeliveryVerificationChargedResourcesV1,
    observed: DeliveryVerificationObservedResourcesV1,
    resource_limit_exceeded: bool,
    calibration_decision: Option<DecisionReceiptV1>,
    holdout_decision: Option<DecisionReceiptV1>,
    terminal: Option<CampaignTerminalReceiptV1>,
    journal_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryReceiptV1 {
    schema: String,
    disposition: DeliveryVerificationCampaignDispositionV1,
    reason: String,
    output_root_sha256: String,
    observed_journal_sha256: Option<String>,
    recovered_at_ms: u64,
    recovery_sha256: String,
}

pub(super) struct DeliveryVerificationExecutionJournal {
    root: PathBuf,
    document: JournalDocumentV1,
    _lock: DeliveryVerificationExecutionLock,
}

impl DeliveryVerificationExecutionJournal {
    pub(super) fn create_new(
        output_root: &Path,
        validated: &ValidatedDeliveryVerificationAuthorizationV1,
        tombstone: &ConsumedDeliveryVerificationAuthorizationV1,
    ) -> Result<Self, String> {
        tombstone.validate_for(&validated.authorization)?;
        let root = canonical_private_root(output_root)?;
        if root != validated.output_root
            || path_sha256(&root) != validated.authorization.output_root_sha256
        {
            return Err("delivery journal root differs from consumed authorization".into());
        }
        let lock = acquire_execution_lock(&root)?;
        if root.join(DELIVERY_EXECUTION_RECOVERY_FILE_NAME).exists()
            || root.join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME).exists()
        {
            return Err("delivery output root already has execution state".into());
        }
        let stored_tombstone = read_consumed_delivery_authorization(&root)?;
        if &stored_tombstone != tombstone {
            return Err("delivery tombstone differs from consumed authorization".into());
        }
        let cases = validated
            .authorization
            .cases
            .iter()
            .map(|binding| JournalCaseV1 {
                binding: binding.clone(),
                state: JournalCaseStateV1::Planned,
                calls: CALL_STAGES
                    .into_iter()
                    .map(|stage| JournalCallV1 {
                        stage,
                        state: JournalCallStateV1::Planned,
                    })
                    .collect(),
            })
            .collect();
        let mut document = JournalDocumentV1 {
            schema: DELIVERY_EXECUTION_JOURNAL_SCHEMA.into(),
            revision: 0,
            authorization: validated.authorization.clone(),
            tombstone_sha256: tombstone.tombstone_sha256.clone(),
            authorization_consumed_at_ms: tombstone.consumed_at_ms,
            phase: JournalPhaseV1::AuthorizationConsumed,
            campaign_reservation: None,
            cases,
            charged: DeliveryVerificationChargedResourcesV1::default(),
            observed: DeliveryVerificationObservedResourcesV1::default(),
            resource_limit_exceeded: false,
            calibration_decision: None,
            holdout_decision: None,
            terminal: None,
            journal_sha256: String::new(),
        };
        document.reseal()?;
        document.validate()?;
        write_new_private_file(
            &root.join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME),
            &canonical_json(&document, "delivery execution journal")?,
            "delivery execution journal",
        )?;
        Ok(Self {
            root,
            document,
            _lock: lock,
        })
    }

    pub(super) fn recover(
        output_root: &Path,
        recovered_at_ms: u64,
    ) -> Result<DeliveryVerificationRecoveryV1, String> {
        let root = canonical_private_root(output_root)?;
        let lock = acquire_execution_lock(&root)?;
        if root.join(DELIVERY_EXECUTION_RECOVERY_FILE_NAME).exists() {
            let receipt = read_recovery_receipt(&root)?;
            drop(lock);
            return Ok(DeliveryVerificationRecoveryV1::RecoveryTerminal(
                receipt.disposition,
            ));
        }
        let journal_path = root.join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME);
        let tombstone = match read_consumed_delivery_authorization(&root) {
            Ok(tombstone) => tombstone,
            Err(_) => {
                let receipt = write_recovery_receipt(
                    &root,
                    DeliveryVerificationCampaignDispositionV1::Invalid,
                    "tampered_or_malformed_tombstone",
                    None,
                    recovered_at_ms,
                )?;
                drop(lock);
                return Ok(DeliveryVerificationRecoveryV1::RecoveryTerminal(
                    receipt.disposition,
                ));
            }
        };
        if !journal_path.exists() {
            let receipt = write_recovery_receipt(
                &root,
                DeliveryVerificationCampaignDispositionV1::Censored,
                "interrupted_before_valid_journal",
                None,
                recovered_at_ms,
            )?;
            drop(lock);
            return Ok(DeliveryVerificationRecoveryV1::RecoveryTerminal(
                receipt.disposition,
            ));
        }
        let bytes = match read_private_file(&journal_path, "delivery execution journal") {
            Ok(bytes) => bytes,
            Err(_) => {
                let receipt = write_recovery_receipt(
                    &root,
                    DeliveryVerificationCampaignDispositionV1::Invalid,
                    "tampered_or_malformed_journal",
                    None,
                    recovered_at_ms,
                )?;
                drop(lock);
                return Ok(DeliveryVerificationRecoveryV1::RecoveryTerminal(
                    receipt.disposition,
                ));
            }
        };
        let observed_digest = sha256_hex(&bytes);
        let parsed = serde_json::from_slice::<JournalDocumentV1>(&bytes)
            .map_err(|error| format!("invalid delivery execution journal JSON: {error}"));
        let document = match parsed.and_then(|document| {
            if canonical_json(&document, "delivery execution journal")? != bytes {
                return Err("delivery execution journal is not canonical JSON".into());
            }
            document.validate()?;
            validate_journal_response_artifacts(&root, &document)?;
            tombstone.validate_for(&document.authorization)?;
            if tombstone.tombstone_sha256 != document.tombstone_sha256
                || path_sha256(&root) != document.authorization.output_root_sha256
            {
                return Err("delivery journal is not anchored to its tombstone".into());
            }
            Ok(document)
        }) {
            Ok(document) => document,
            Err(_) => {
                let receipt = write_recovery_receipt(
                    &root,
                    DeliveryVerificationCampaignDispositionV1::Invalid,
                    "tampered_or_malformed_journal",
                    Some(observed_digest),
                    recovered_at_ms,
                )?;
                drop(lock);
                return Ok(DeliveryVerificationRecoveryV1::RecoveryTerminal(
                    receipt.disposition,
                ));
            }
        };
        if let Some(terminal) = &document.terminal {
            let disposition = terminal.disposition;
            drop(lock);
            return Ok(DeliveryVerificationRecoveryV1::Terminal(disposition));
        }
        if document.campaign_reservation.is_none() {
            let receipt = write_recovery_receipt(
                &root,
                DeliveryVerificationCampaignDispositionV1::Censored,
                "interrupted_before_campaign_reservation",
                Some(observed_digest),
                recovered_at_ms,
            )?;
            drop(lock);
            return Ok(DeliveryVerificationRecoveryV1::RecoveryTerminal(
                receipt.disposition,
            ));
        }
        let mut journal = Self {
            root,
            document,
            _lock: lock,
        };
        let recovery_reason = if recovered_at_ms > campaign_deadline(&journal.document)? {
            "campaign_timeout_exceeded"
        } else {
            "interrupted_execution"
        };
        journal.freeze(
            recovery_reason,
            DeliveryVerificationCampaignDispositionV1::Censored,
            recovered_at_ms,
        )?;
        let disposition = journal
            .document
            .terminal
            .as_ref()
            .expect("freeze writes a terminal receipt")
            .disposition;
        drop(journal);
        Ok(DeliveryVerificationRecoveryV1::RecoveryTerminal(
            disposition,
        ))
    }

    pub(super) fn reserve_campaign(&mut self, reserved_at_ms: u64) -> Result<(), String> {
        self.transition(|document| {
            if document.phase != JournalPhaseV1::AuthorizationConsumed
                || document.campaign_reservation.is_some()
                || reserved_at_ms < document.authorization_consumed_at_ms
                || reserved_at_ms >= document.authorization.expires_at_ms
            {
                return Err("delivery campaign reservation is invalid or out of order".into());
            }
            let budget = &document.authorization.budget;
            let mut reservation = CampaignReservationReceiptV1 {
                max_logical_model_calls: budget.max_logical_model_calls_total,
                max_physical_model_attempts: budget.max_physical_model_attempts_total,
                max_total_tokens: budget.max_total_tokens_campaign,
                max_duration_ms: budget.campaign_timeout_ms,
                reserved_at_ms,
                reservation_sha256: String::new(),
            };
            reservation.reservation_sha256 = campaign_reservation_digest(&reservation)?;
            document.campaign_reservation = Some(reservation);
            document.phase = JournalPhaseV1::CampaignReserved;
            Ok(())
        })
    }

    pub(super) fn reserve_case(
        &mut self,
        ordinal: usize,
        reserved_at_ms: u64,
    ) -> Result<(), String> {
        self.transition(|document| {
            require_live(document)?;
            let index = case_index(document, ordinal)?;
            if reserved_at_ms == 0
                || !matches!(document.cases[index].state, JournalCaseStateV1::Planned)
                || document.cases[..index]
                    .iter()
                    .any(|case| !matches!(case.state, JournalCaseStateV1::Terminal { .. }))
                || (ordinal > CALIBRATION_CASES
                    && document
                        .calibration_decision
                        .as_ref()
                        .is_none_or(|decision| decision.decision != "open_holdout"))
            {
                return Err("delivery case reservation is invalid or out of order".into());
            }
            document.cases[index].state = JournalCaseStateV1::Reserved { reserved_at_ms };
            document.phase = JournalPhaseV1::Executing;
            Ok(())
        })
    }

    pub(super) fn reserve_call(
        &mut self,
        input: DeliveryVerificationCallReservationInputV1,
    ) -> Result<DeliveryVerificationCallPermitV1, String> {
        let index = case_index(&self.document, input.case_ordinal)?;
        let stage_index = stage_index(input.stage);
        let global_call_ordinal = usize::try_from(self.document.charged.logical_model_calls)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| "delivery global call ordinal overflowed".to_string())?;
        let role = role_label(&input.role)?;
        validate_call_binding(&self.document, &input, &role)?;
        let mut reservation = CallReservationReceiptV1 {
            case_ordinal: input.case_ordinal,
            global_call_ordinal,
            stage: input.stage,
            role,
            configured_model_sha256: input.configured_model_sha256,
            semantic_request_sha256: input.semantic_request_sha256,
            semantic_request_bytes: input.semantic_request_bytes,
            wire_payload_sha256: input.wire_payload_sha256,
            wire_payload_bytes: input.wire_payload_bytes,
            max_output_tokens: input.max_output_tokens,
            reserved_at_ms: input.reserved_at_ms,
            reservation_sha256: String::new(),
        };
        reservation.reservation_sha256 = call_reservation_digest(&reservation)?;
        let permit = DeliveryVerificationCallPermitV1 {
            case_ordinal: reservation.case_ordinal,
            global_call_ordinal,
            stage: reservation.stage,
            reservation_sha256: reservation.reservation_sha256.clone(),
        };
        self.transition(|document| {
            require_live(document)?;
            if !matches!(
                document.cases[index].state,
                JournalCaseStateV1::Reserved { .. }
            ) || !matches!(
                document.cases[index].calls[stage_index].state,
                JournalCallStateV1::Planned
            ) || document.cases[index].calls[..stage_index]
                .iter()
                .any(|call| {
                    !matches!(
                        &call.state,
                        JournalCallStateV1::Terminal { receipt, .. }
                            if receipt.status
                                == DeliveryVerificationCallTerminalStatusV1::Completed
                    )
                })
            {
                return Err("delivery call reservation is invalid or out of order".into());
            }
            let next_calls = document
                .charged
                .logical_model_calls
                .checked_add(1)
                .ok_or_else(|| "delivery logical-call accounting overflowed".to_string())?;
            let next_attempts = document
                .charged
                .physical_model_attempts
                .checked_add(1)
                .ok_or_else(|| "delivery physical-attempt accounting overflowed".to_string())?;
            let next_output = document
                .charged
                .reserved_output_tokens
                .checked_add(reservation.max_output_tokens)
                .ok_or_else(|| "delivery output-token reservation overflowed".to_string())?;
            if next_calls > document.authorization.budget.max_logical_model_calls_total as u64
                || next_attempts
                    > document
                        .authorization
                        .budget
                        .max_physical_model_attempts_total as u64
            {
                return Err("delivery call reservation exceeds the frozen campaign budget".into());
            }
            document.charged = DeliveryVerificationChargedResourcesV1 {
                logical_model_calls: next_calls,
                physical_model_attempts: next_attempts,
                reserved_output_tokens: next_output,
            };
            document.cases[index].calls[stage_index].state =
                JournalCallStateV1::Reserved { reservation };
            Ok(())
        })?;
        Ok(permit)
    }

    pub(super) fn record_call_terminal(
        &mut self,
        input: DeliveryVerificationCallTerminalInputV1,
        response_artifact: Option<&[u8]>,
    ) -> Result<Option<DeliveryVerificationResponseArtifactV1>, String> {
        let reservation = self.reservation_for_permit(&input.permit)?.clone();
        let artifact = response_artifact.map(|bytes| DeliveryVerificationResponseArtifactV1 {
            sha256: sha256_hex(bytes),
            bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            path: self.root.join(format!(
                "case-{:02}-call-{:03}-response.bin",
                reservation.case_ordinal, reservation.global_call_ordinal
            )),
        });
        let mut receipt = CallTerminalReceiptV1 {
            status: input.status,
            failure_class: input.failure_class,
            retryable: input.retryable,
            provider_status_code: input.provider_status_code,
            latency_ms: input.latency_ms,
            response_artifact_sha256: artifact.as_ref().map(|value| value.sha256.clone()),
            response_artifact_bytes: artifact.as_ref().map(|value| value.bytes),
            request_payload_sha256: input.request_payload_sha256,
            response_semantic_sha256: input.response_semantic_sha256,
            provider_response_id_sha256: input.provider_response_id_sha256,
            provider_response_model_sha256: input.provider_response_model_sha256,
            provider_system_fingerprint_sha256: input.provider_system_fingerprint_sha256,
            provider_receipt_status: input.provider_receipt_status,
            usage: input.usage,
            terminal_at_ms: input.terminal_at_ms,
            terminal_sha256: String::new(),
        };
        validate_call_terminal(&reservation, &receipt)?;
        receipt.terminal_sha256 = call_terminal_digest(&receipt)?;
        let case_index = reservation.case_ordinal - 1;
        let call_index = stage_index(reservation.stage);
        let terminal_at_ms = input.terminal_at_ms;
        let stored_reservation = reservation.clone();
        let stored_receipt = receipt.clone();
        let next = self.prepare_transition(|document| {
            require_live(document)?;
            let campaign_timed_out = terminal_at_ms > campaign_deadline(document)?;
            let current = &document.cases[case_index].calls[call_index].state;
            if !matches!(
                current,
                JournalCallStateV1::Reserved { reservation: value } if value == &reservation
            ) {
                return Err("delivery call terminal does not match its durable reservation".into());
            }
            document.observed.terminal_model_calls = document
                .observed
                .terminal_model_calls
                .checked_add(1)
                .ok_or_else(|| "delivery terminal-call accounting overflowed".to_string())?;
            document.observed.latency_ms = document
                .observed
                .latency_ms
                .checked_add(receipt.latency_ms)
                .ok_or_else(|| "delivery latency accounting overflowed".to_string())?;
            if let Some(usage) = &receipt.usage {
                document.observed.prompt_tokens = document
                    .observed
                    .prompt_tokens
                    .checked_add(usage.prompt_tokens)
                    .ok_or_else(|| "delivery prompt-token accounting overflowed".to_string())?;
                document.observed.completion_tokens = document
                    .observed
                    .completion_tokens
                    .checked_add(usage.completion_tokens)
                    .ok_or_else(|| "delivery completion-token accounting overflowed".to_string())?;
                document.observed.total_tokens = document
                    .observed
                    .total_tokens
                    .checked_add(usage.total_tokens)
                    .ok_or_else(|| "delivery token accounting overflowed".to_string())?;
            }
            document.cases[case_index].calls[call_index].state = JournalCallStateV1::Terminal {
                reservation: stored_reservation,
                receipt: Box::new(stored_receipt),
            };
            let case_tokens = case_resources(&document.cases[case_index])?.total_tokens;
            let resource_limit_exceeded = case_tokens
                > document.authorization.budget.max_total_tokens_per_case
                || document.observed.total_tokens
                    > document.authorization.budget.max_total_tokens_campaign;
            if resource_limit_exceeded {
                document.resource_limit_exceeded = true;
            }
            if campaign_timed_out {
                terminalize(
                    document,
                    DeliveryVerificationCampaignDispositionV1::Inconclusive,
                    "campaign_timeout_exceeded",
                    campaign_timeout_evidence_sha256(),
                    terminal_at_ms,
                )?;
            } else if resource_limit_exceeded {
                terminalize(
                    document,
                    DeliveryVerificationCampaignDispositionV1::Inconclusive,
                    "resource_budget_exceeded",
                    sha256_hex(b"resource_budget_exceeded"),
                    terminal_at_ms,
                )?;
            }
            Ok(())
        })?;
        if let (Some(artifact), Some(bytes)) = (&artifact, response_artifact) {
            write_new_private_file(&artifact.path, bytes, "delivery response artifact")?;
            if let Err(error) = validate_response_artifact(&self.root, &reservation, &receipt) {
                let _ = fs::remove_file(&artifact.path);
                return Err(error);
            }
        }
        if let Err(error) = self.commit_transition(next) {
            if let Some(artifact) = &artifact {
                let _ = fs::remove_file(&artifact.path);
            }
            return Err(error);
        }
        Ok(artifact)
    }

    pub(super) fn record_case_terminal(
        &mut self,
        input: DeliveryVerificationCaseTerminalInputV1,
    ) -> Result<(), String> {
        if input.status == DeliveryVerificationCaseTerminalStatusV1::Complete
            && input.completed_at_ms > campaign_deadline(&self.document)?
        {
            return Err("complete delivery case exceeded the campaign timeout".into());
        }
        let index = case_index(&self.document, input.case_ordinal)?;
        let resources = case_resources(&self.document.cases[index])?;
        let mut receipt = CaseTerminalReceiptV1 {
            status: input.status,
            control_passed: input.control_passed,
            treatment_passed: input.treatment_passed,
            seeded_candidate_sha256: input.seeded_candidate_sha256,
            seeded_candidate_bytes: input.seeded_candidate_bytes,
            control_output_sha256: input.control_output_sha256,
            treatment_output_sha256: input.treatment_output_sha256,
            initial_verifier_decision: input.initial_verifier_decision,
            initial_finding_counts: input.initial_finding_counts,
            repair_activated: input.repair_activated,
            recheck_decision: input.recheck_decision,
            recheck_finding_counts: input.recheck_finding_counts,
            treatment_disposition: input.treatment_disposition,
            failure_stage: input.failure_stage,
            failure_code: input.failure_code,
            outcome_reason: input.outcome_reason,
            observation_sha256: input.observation_sha256,
            resources,
            completed_at_ms: input.completed_at_ms,
            receipt_sha256: String::new(),
        };
        receipt.receipt_sha256 = case_terminal_digest(&receipt)?;
        validate_case_terminal(&self.document.cases[index], &receipt)?;
        self.transition(|document| {
            require_live(document)?;
            let case = &mut document.cases[index];
            if !matches!(case.state, JournalCaseStateV1::Reserved { .. })
                || case
                    .calls
                    .iter()
                    .any(|call| matches!(call.state, JournalCallStateV1::Reserved { .. }))
            {
                return Err("delivery case terminal has a pending or unreserved case".into());
            }
            for call in &mut case.calls {
                if matches!(call.state, JournalCallStateV1::Planned) {
                    call.state = JournalCallStateV1::NotRequired {
                        reason: format!("case_terminal:{:?}", receipt.status).to_ascii_lowercase(),
                        closed_at_ms: receipt.completed_at_ms,
                    };
                }
            }
            case.state = JournalCaseStateV1::Terminal { receipt };
            Ok(())
        })
    }

    pub(super) fn record_calibration_decision(
        &mut self,
        decision: &str,
        counts_sha256: String,
        decided_at_ms: u64,
    ) -> Result<(), String> {
        if !matches!(
            decision,
            "open_holdout" | "terminal_futility" | "inconclusive" | "invalid"
        ) {
            return Err("delivery calibration decision is invalid".into());
        }
        let receipt = decision_receipt("calibration", decision, counts_sha256, decided_at_ms)?;
        self.transition(|document| {
            require_live(document)?;
            if decided_at_ms > campaign_deadline(document)? {
                return Err("delivery calibration decision exceeded the campaign timeout".into());
            }
            if document.calibration_decision.is_some()
                || document.cases[..CALIBRATION_CASES]
                    .iter()
                    .any(|case| !matches!(case.state, JournalCaseStateV1::Terminal { .. }))
            {
                return Err("delivery calibration decision is premature or duplicated".into());
            }
            if decision != "open_holdout" {
                for case in &mut document.cases[CALIBRATION_CASES..] {
                    if !matches!(case.state, JournalCaseStateV1::Planned) {
                        return Err("delivery holdout was touched before calibration closed".into());
                    }
                    case.state = JournalCaseStateV1::Skipped {
                        reason: format!("calibration_closed:{decision}"),
                        closed_at_ms: decided_at_ms,
                    };
                }
            }
            document.calibration_decision = Some(receipt);
            Ok(())
        })
    }

    pub(super) fn record_holdout_decision(
        &mut self,
        decision: &str,
        counts_sha256: String,
        decided_at_ms: u64,
    ) -> Result<(), String> {
        if !matches!(
            decision,
            "seeded_repair_effective"
                | "not_effective"
                | "preservation_regression"
                | "inconclusive"
                | "invalid"
        ) {
            return Err("delivery holdout decision is invalid".into());
        }
        let receipt = decision_receipt("holdout", decision, counts_sha256, decided_at_ms)?;
        self.transition(|document| {
            require_live(document)?;
            if decided_at_ms > campaign_deadline(document)? {
                return Err("delivery holdout decision exceeded the campaign timeout".into());
            }
            if document.holdout_decision.is_some()
                || document
                    .calibration_decision
                    .as_ref()
                    .is_none_or(|value| value.decision != "open_holdout")
                || document.cases[CALIBRATION_CASES..]
                    .iter()
                    .any(|case| !matches!(case.state, JournalCaseStateV1::Terminal { .. }))
            {
                return Err("delivery holdout decision is premature or duplicated".into());
            }
            document.holdout_decision = Some(receipt);
            Ok(())
        })
    }

    pub(super) fn finish(
        &mut self,
        disposition: DeliveryVerificationCampaignDispositionV1,
        reason: &str,
        evidence_sha256: String,
        terminal_at_ms: u64,
    ) -> Result<(), String> {
        require_sha256(&evidence_sha256, "delivery campaign evidence")?;
        if reason.trim().is_empty() || terminal_at_ms == 0 {
            return Err("delivery campaign terminal receipt is incomplete".into());
        }
        self.transition(|document| {
            require_live(document)?;
            if terminal_at_ms > campaign_deadline(document)?
                && (!matches!(
                    disposition,
                    DeliveryVerificationCampaignDispositionV1::Inconclusive
                        | DeliveryVerificationCampaignDispositionV1::Censored
                ) || reason != "campaign_timeout_exceeded")
            {
                return Err("late delivery terminal is not a timeout terminal".into());
            }
            validate_finish_state(document, disposition)?;
            terminalize(
                document,
                disposition,
                reason,
                evidence_sha256,
                terminal_at_ms,
            )
        })
    }

    pub(super) fn freeze(
        &mut self,
        reason: &str,
        disposition: DeliveryVerificationCampaignDispositionV1,
        terminal_at_ms: u64,
    ) -> Result<(), String> {
        if matches!(
            disposition,
            DeliveryVerificationCampaignDispositionV1::SeededRepairEffective
                | DeliveryVerificationCampaignDispositionV1::NotEffective
                | DeliveryVerificationCampaignDispositionV1::PreservationRegression
        ) {
            return Err("delivery freeze cannot claim a completed holdout decision".into());
        }
        let evidence_sha256 =
            sha256_hex(format!("cindx.delivery-verification-freeze.v3\0{reason}").as_bytes());
        self.finish(disposition, reason, evidence_sha256, terminal_at_ms)
    }

    #[cfg(test)]
    pub(super) fn charged(&self) -> DeliveryVerificationChargedResourcesV1 {
        self.document.charged
    }

    #[cfg(test)]
    pub(super) fn observed(&self) -> DeliveryVerificationObservedResourcesV1 {
        self.document.observed
    }

    pub(super) fn is_terminal(&self) -> bool {
        self.document.terminal.is_some()
    }

    fn reservation_for_permit(
        &self,
        permit: &DeliveryVerificationCallPermitV1,
    ) -> Result<&CallReservationReceiptV1, String> {
        let index = case_index(&self.document, permit.case_ordinal)?;
        let call = &self.document.cases[index].calls[stage_index(permit.stage)];
        match &call.state {
            JournalCallStateV1::Reserved { reservation }
                if reservation.global_call_ordinal == permit.global_call_ordinal
                    && reservation.reservation_sha256 == permit.reservation_sha256 =>
            {
                Ok(reservation)
            }
            _ => Err("delivery call permit is stale, used, or not durably reserved".into()),
        }
    }

    fn transition(
        &mut self,
        apply: impl FnOnce(&mut JournalDocumentV1) -> Result<(), String>,
    ) -> Result<(), String> {
        let next = self.prepare_transition(apply)?;
        self.commit_transition(next)
    }

    fn prepare_transition(
        &self,
        apply: impl FnOnce(&mut JournalDocumentV1) -> Result<(), String>,
    ) -> Result<JournalDocumentV1, String> {
        if self.document.terminal.is_some() {
            return Err("delivery execution journal is terminal".into());
        }
        require_private_directory(&self.root, "delivery output root")?;
        let mut next = self.document.clone();
        apply(&mut next)?;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| "delivery journal revision overflowed".to_string())?;
        next.reseal()?;
        next.validate()?;
        Ok(next)
    }

    fn commit_transition(&mut self, next: JournalDocumentV1) -> Result<(), String> {
        require_private_directory(&self.root, "delivery output root")?;
        replace_private_file(
            &self.root.join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME),
            &canonical_json(&next, "delivery execution journal")?,
            "delivery execution journal",
        )?;
        self.document = next;
        Ok(())
    }
}
