use super::*;
use agent_runtime::MAX_DELIVERY_VERIFICATION_FINDINGS;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

impl JournalDocumentV1 {
    pub(super) fn reseal(&mut self) -> Result<(), String> {
        self.journal_sha256.clear();
        self.journal_sha256 = journal_digest(self)?;
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        self.authorization.validate_static()?;
        require_sha256(&self.tombstone_sha256, "delivery consumed tombstone")?;
        if self.schema != DELIVERY_EXECUTION_JOURNAL_SCHEMA
            || self.cases.len() != CASE_COUNT
            || self.authorization_consumed_at_ms < self.authorization.issued_at_ms
            || self.authorization_consumed_at_ms >= self.authorization.expires_at_ms
            || self.journal_sha256 != journal_digest(self)?
        {
            return Err("delivery execution journal envelope is invalid".into());
        }
        match (&self.phase, &self.campaign_reservation) {
            (JournalPhaseV1::AuthorizationConsumed, None) => {}
            (JournalPhaseV1::CampaignReserved | JournalPhaseV1::Executing, Some(value))
            | (JournalPhaseV1::Terminal, Some(value)) => validate_campaign_reservation(
                value,
                &self.authorization,
                self.authorization_consumed_at_ms,
            )?,
            _ => return Err("delivery campaign reservation phase is inconsistent".into()),
        }
        let campaign_deadline = self
            .campaign_reservation
            .as_ref()
            .map(campaign_deadline_for_reservation)
            .transpose()?;
        let mut charged = DeliveryVerificationChargedResourcesV1::default();
        let mut observed = DeliveryVerificationObservedResourcesV1::default();
        let mut global_ordinals = BTreeSet::new();
        let mut late_call_terminal_at_ms = None;
        for (index, case) in self.cases.iter().enumerate() {
            if case.binding != self.authorization.cases[index]
                || case.calls.len() != CALL_STAGES.len()
                || case
                    .calls
                    .iter()
                    .zip(CALL_STAGES)
                    .any(|(call, stage)| call.stage != stage)
            {
                return Err("delivery journal case binding is invalid".into());
            }
            match &case.state {
                JournalCaseStateV1::Planned => {
                    if case
                        .calls
                        .iter()
                        .any(|call| !matches!(call.state, JournalCallStateV1::Planned))
                    {
                        return Err("planned delivery case contains call state".into());
                    }
                }
                JournalCaseStateV1::Reserved { reserved_at_ms } => {
                    if *reserved_at_ms == 0 {
                        return Err("reserved delivery case has no timestamp".into());
                    }
                }
                JournalCaseStateV1::Terminal { receipt } => {
                    validate_case_terminal(case, receipt)?;
                    if receipt.status == DeliveryVerificationCaseTerminalStatusV1::Complete
                        && campaign_deadline
                            .is_some_and(|deadline| receipt.completed_at_ms > deadline)
                    {
                        return Err("complete delivery case exceeded the campaign timeout".into());
                    }
                    if case.calls.iter().any(|call| {
                        matches!(
                            call.state,
                            JournalCallStateV1::Planned | JournalCallStateV1::Reserved { .. }
                        )
                    }) {
                        return Err("terminal delivery case has an open call slot".into());
                    }
                }
                JournalCaseStateV1::Skipped {
                    reason,
                    closed_at_ms,
                } => {
                    if reason.trim().is_empty()
                        || *closed_at_ms == 0
                        || case
                            .calls
                            .iter()
                            .any(|call| !matches!(call.state, JournalCallStateV1::Planned))
                    {
                        return Err("skipped delivery case is invalid".into());
                    }
                }
            }
            for call in &case.calls {
                match &call.state {
                    JournalCallStateV1::Planned => {}
                    JournalCallStateV1::Reserved { reservation } => {
                        validate_stored_reservation(reservation, &self.authorization)?;
                        accumulate_charge(&mut charged, reservation)?;
                        if !global_ordinals.insert(reservation.global_call_ordinal) {
                            return Err("delivery journal repeats a global call ordinal".into());
                        }
                    }
                    JournalCallStateV1::Terminal {
                        reservation,
                        receipt,
                    } => {
                        validate_stored_reservation(reservation, &self.authorization)?;
                        validate_call_terminal(reservation, receipt)?;
                        if receipt.terminal_sha256 != call_terminal_digest(receipt)? {
                            return Err("delivery call terminal digest is invalid".into());
                        }
                        if campaign_deadline
                            .is_some_and(|deadline| receipt.terminal_at_ms > deadline)
                            && late_call_terminal_at_ms
                                .replace(receipt.terminal_at_ms)
                                .is_some()
                        {
                            return Err(
                                "delivery journal contains multiple late call terminals".into()
                            );
                        }
                        accumulate_charge(&mut charged, reservation)?;
                        accumulate_observed(&mut observed, receipt)?;
                        if !global_ordinals.insert(reservation.global_call_ordinal) {
                            return Err("delivery journal repeats a global call ordinal".into());
                        }
                    }
                    JournalCallStateV1::NotRequired {
                        reason,
                        closed_at_ms,
                    } => {
                        if reason.trim().is_empty() || *closed_at_ms == 0 {
                            return Err("delivery unused call slot is invalid".into());
                        }
                    }
                }
            }
        }
        if global_ordinals
            != (1..=usize::try_from(charged.logical_model_calls).unwrap_or(0))
                .collect::<BTreeSet<_>>()
            || charged != self.charged
            || observed != self.observed
        {
            return Err("delivery execution resource ledger is invalid".into());
        }
        if charged.logical_model_calls
            > self.authorization.budget.max_logical_model_calls_total as u64
            || charged.physical_model_attempts
                > self.authorization.budget.max_physical_model_attempts_total as u64
        {
            return Err("delivery charged resources exceed the frozen budget".into());
        }
        let observed_overflow = observed.total_tokens
            > self.authorization.budget.max_total_tokens_campaign
            || self.cases.iter().any(|case| {
                case_resources(case).is_ok_and(|resources| {
                    resources.total_tokens > self.authorization.budget.max_total_tokens_per_case
                })
            });
        if observed_overflow != self.resource_limit_exceeded {
            return Err("delivery resource overflow marker is inconsistent".into());
        }
        if let Some(decision) = &self.calibration_decision {
            validate_decision(decision, "calibration")?;
            if campaign_deadline.is_some_and(|deadline| decision.decided_at_ms > deadline) {
                return Err("delivery calibration decision exceeded the campaign timeout".into());
            }
        }
        if let Some(decision) = &self.holdout_decision {
            validate_decision(decision, "holdout")?;
            if campaign_deadline.is_some_and(|deadline| decision.decided_at_ms > deadline) {
                return Err("delivery holdout decision exceeded the campaign timeout".into());
            }
        }
        match (&self.phase, &self.terminal) {
            (JournalPhaseV1::Terminal, Some(terminal)) => {
                require_sha256(&terminal.evidence_sha256, "delivery terminal evidence")?;
                if terminal.reason.trim().is_empty()
                    || terminal.terminal_at_ms == 0
                    || terminal.terminal_sha256 != terminal_digest(terminal)?
                {
                    return Err("delivery terminal receipt is invalid".into());
                }
                if campaign_deadline.is_some_and(|deadline| terminal.terminal_at_ms > deadline)
                    && (!matches!(
                        terminal.disposition,
                        DeliveryVerificationCampaignDispositionV1::Inconclusive
                            | DeliveryVerificationCampaignDispositionV1::Censored
                    ) || terminal.reason != "campaign_timeout_exceeded")
                {
                    return Err("late delivery terminal is not a timeout terminal".into());
                }
                if late_call_terminal_at_ms.is_some_and(|terminal_at_ms| {
                    terminal.disposition != DeliveryVerificationCampaignDispositionV1::Inconclusive
                        || terminal.reason != "campaign_timeout_exceeded"
                        || terminal.evidence_sha256 != campaign_timeout_evidence_sha256()
                        || terminal.terminal_at_ms != terminal_at_ms
                }) {
                    return Err("late delivery call lacks its immediate timeout terminal".into());
                }
            }
            (JournalPhaseV1::Terminal, None) | (_, Some(_)) => {
                return Err("delivery terminal phase is inconsistent".into())
            }
            _ => {}
        }
        Ok(())
    }
}

pub(super) fn validate_call_binding(
    document: &JournalDocumentV1,
    input: &DeliveryVerificationCallReservationInputV1,
    role: &str,
) -> Result<(), String> {
    let case = document
        .cases
        .get(input.case_ordinal.saturating_sub(1))
        .ok_or_else(|| "delivery call case is outside the frozen suite".to_string())?;
    if case.binding.ordinal != input.case_ordinal
        || input.reserved_at_ms == 0
        || input.semantic_request_bytes == 0
        || input.semantic_request_bytes > MAX_SEMANTIC_REQUEST_BYTES
        || input.wire_payload_bytes == 0
        || input.wire_payload_bytes > MAX_WIRE_PAYLOAD_BYTES
        || document
            .campaign_reservation
            .as_ref()
            .is_none_or(|campaign| {
                input.reserved_at_ms < campaign.reserved_at_ms
                    || campaign
                        .reserved_at_ms
                        .checked_add(campaign.max_duration_ms)
                        .is_none_or(|deadline| input.reserved_at_ms > deadline)
            })
    {
        return Err("delivery call reservation input is invalid".into());
    }
    require_sha256(&input.configured_model_sha256, "delivery configured model")?;
    require_sha256(&input.semantic_request_sha256, "delivery semantic request")?;
    require_sha256(&input.wire_payload_sha256, "delivery wire payload")?;
    let (expected_role, expected_model, expected_output) = match input.stage {
        DeliveryVerificationCallStageV1::OwnerRepair => (
            "executor",
            &document.authorization.provider.owner_model_sha256,
            document.authorization.budget.max_owner_output_tokens,
        ),
        DeliveryVerificationCallStageV1::VerifierInitial
        | DeliveryVerificationCallStageV1::VerifierRecheck => (
            "reviewer",
            &document.authorization.provider.verifier_model_sha256,
            document.authorization.budget.max_verifier_output_tokens,
        ),
    };
    if role != expected_role
        || &input.configured_model_sha256 != expected_model
        || input.max_output_tokens != expected_output
    {
        return Err("delivery call role, model, or output budget differs from authority".into());
    }
    Ok(())
}

pub(super) fn validate_stored_reservation(
    reservation: &CallReservationReceiptV1,
    authorization: &DeliveryVerificationAuthorizationV1,
) -> Result<(), String> {
    if reservation.case_ordinal == 0
        || reservation.case_ordinal > authorization.cases.len()
        || reservation.global_call_ordinal == 0
        || reservation.reserved_at_ms == 0
        || reservation.semantic_request_bytes == 0
        || reservation.semantic_request_bytes > MAX_SEMANTIC_REQUEST_BYTES
        || reservation.wire_payload_bytes == 0
        || reservation.wire_payload_bytes > MAX_WIRE_PAYLOAD_BYTES
        || reservation.reservation_sha256 != call_reservation_digest(reservation)?
    {
        return Err("delivery stored call reservation is invalid".into());
    }
    require_sha256(
        &reservation.configured_model_sha256,
        "delivery configured model",
    )?;
    require_sha256(
        &reservation.semantic_request_sha256,
        "delivery semantic request",
    )?;
    require_sha256(&reservation.wire_payload_sha256, "delivery wire payload")?;
    let (role, model, output) = match reservation.stage {
        DeliveryVerificationCallStageV1::OwnerRepair => (
            "executor",
            &authorization.provider.owner_model_sha256,
            authorization.budget.max_owner_output_tokens,
        ),
        DeliveryVerificationCallStageV1::VerifierInitial
        | DeliveryVerificationCallStageV1::VerifierRecheck => (
            "reviewer",
            &authorization.provider.verifier_model_sha256,
            authorization.budget.max_verifier_output_tokens,
        ),
    };
    if reservation.role != role
        || &reservation.configured_model_sha256 != model
        || reservation.max_output_tokens != output
    {
        return Err("delivery stored call authority is invalid".into());
    }
    Ok(())
}

pub(super) fn validate_call_terminal(
    reservation: &CallReservationReceiptV1,
    receipt: &CallTerminalReceiptV1,
) -> Result<(), String> {
    if receipt.terminal_at_ms < reservation.reserved_at_ms
        || receipt
            .provider_status_code
            .is_some_and(|value| !(100..=599).contains(&value))
    {
        return Err("delivery call terminal time or status code is invalid".into());
    }
    for value in [
        receipt.response_artifact_sha256.as_deref(),
        receipt.request_payload_sha256.as_deref(),
        receipt.response_semantic_sha256.as_deref(),
        receipt.provider_response_id_sha256.as_deref(),
        receipt.provider_response_model_sha256.as_deref(),
        receipt.provider_system_fingerprint_sha256.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        require_sha256(value, "delivery call receipt digest")?;
    }
    if receipt.response_artifact_sha256.is_some() != receipt.response_artifact_bytes.is_some()
        || receipt.response_artifact_bytes == Some(0)
    {
        return Err("delivery response artifact receipt is inconsistent".into());
    }
    if let Some(usage) = &receipt.usage {
        if usage
            .prompt_tokens
            .checked_add(usage.completion_tokens)
            .is_none()
            || usage.prompt_tokens + usage.completion_tokens != usage.total_tokens
            || usage.usage_source.trim().is_empty()
        {
            return Err("delivery call usage receipt is invalid".into());
        }
    }
    match receipt.status {
        DeliveryVerificationCallTerminalStatusV1::Completed => {
            if receipt.failure_class.is_some()
                || receipt.retryable.is_some()
                || receipt.response_artifact_sha256.is_none()
                || receipt.request_payload_sha256.is_none()
                || receipt.request_payload_sha256.as_ref() != Some(&reservation.wire_payload_sha256)
                || receipt.response_semantic_sha256.is_none()
                || receipt.provider_response_id_sha256.is_none()
                || receipt.provider_response_model_sha256.is_none()
                || receipt.provider_response_model_sha256.as_ref()
                    != Some(&reservation.configured_model_sha256)
                || receipt.provider_receipt_status.as_deref() != Some("observed")
                || receipt
                    .usage
                    .as_ref()
                    .is_none_or(|usage| usage.usage_source != "provider" || usage.usage_estimated)
            {
                return Err("completed delivery call lacks exact provider receipts".into());
            }
        }
        _ => {
            if receipt
                .failure_class
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
                || receipt.retryable.is_none()
            {
                return Err("failed delivery call lacks retained failure classification".into());
            }
        }
    }
    Ok(())
}

pub(super) fn validate_response_artifact(
    root: &Path,
    reservation: &CallReservationReceiptV1,
    receipt: &CallTerminalReceiptV1,
) -> Result<(), String> {
    let (Some(expected_sha256), Some(expected_bytes)) = (
        receipt.response_artifact_sha256.as_ref(),
        receipt.response_artifact_bytes,
    ) else {
        return Ok(());
    };
    let path = root.join(format!(
        "case-{:02}-call-{:03}-response.bin",
        reservation.case_ordinal, reservation.global_call_ordinal
    ));
    let bytes = read_private_file(&path, "delivery response artifact")?;
    if sha256_hex(&bytes) != *expected_sha256
        || u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_bytes
    {
        return Err("delivery response artifact differs from its terminal receipt".into());
    }
    Ok(())
}

pub(super) fn validate_journal_response_artifacts(
    root: &Path,
    document: &JournalDocumentV1,
) -> Result<(), String> {
    for case in &document.cases {
        for call in &case.calls {
            if let JournalCallStateV1::Terminal {
                reservation,
                receipt,
            } = &call.state
            {
                validate_response_artifact(root, reservation, receipt)?;
            }
        }
    }
    Ok(())
}

pub(super) fn validate_case_terminal(
    case: &JournalCaseV1,
    receipt: &CaseTerminalReceiptV1,
) -> Result<(), String> {
    require_sha256(&receipt.observation_sha256, "delivery case observation")?;
    if receipt.completed_at_ms == 0
        || receipt.outcome_reason.trim().is_empty()
        || receipt.receipt_sha256 != case_terminal_digest(receipt)?
    {
        return Err("delivery case terminal receipt is invalid".into());
    }
    for digest in [
        receipt.seeded_candidate_sha256.as_deref(),
        receipt.control_output_sha256.as_deref(),
        receipt.treatment_output_sha256.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        require_sha256(digest, "delivery case output")?;
    }
    if receipt.seeded_candidate_sha256.as_deref()
        != Some(case.binding.seeded_candidate_sha256.as_str())
        || receipt.seeded_candidate_bytes != Some(case.binding.seeded_candidate_bytes)
        || receipt.seeded_candidate_bytes == Some(0)
        || receipt.control_output_sha256 != receipt.seeded_candidate_sha256
        || receipt.resources != case_resources(case)?
    {
        return Err("delivery case output or resource receipt is inconsistent".into());
    }
    validate_case_call_sequence(case, receipt.completed_at_ms)?;
    validate_finding_telemetry(
        receipt.initial_verifier_decision.as_deref(),
        receipt.initial_finding_counts,
    )?;
    validate_finding_telemetry(
        receipt.recheck_decision.as_deref(),
        receipt.recheck_finding_counts,
    )?;
    match receipt.status {
        DeliveryVerificationCaseTerminalStatusV1::Complete => {
            validate_complete_case_terminal(case, receipt)?
        }
        _ => validate_failed_case_terminal(receipt)?,
    }
    Ok(())
}

fn validate_complete_case_terminal(
    case: &JournalCaseV1,
    receipt: &CaseTerminalReceiptV1,
) -> Result<(), String> {
    if receipt.control_passed.is_none() || receipt.treatment_passed.is_none() {
        return Err("complete delivery case lacks matched-pair outcomes".into());
    }
    let initial_decision = receipt.initial_verifier_decision.as_deref();
    if receipt.repair_activated != (initial_decision == Some("needs_revision"))
        || receipt.recheck_decision.is_some() && !receipt.repair_activated
        || initial_decision.is_some()
            && completed_call_receipt(case, DeliveryVerificationCallStageV1::VerifierInitial)
                .is_none()
    {
        return Err("complete delivery case activation telemetry is inconsistent".into());
    }

    match initial_decision {
        Some("passed") => {
            if !call_is_unneeded(case, DeliveryVerificationCallStageV1::OwnerRepair)
                || !call_is_unneeded(case, DeliveryVerificationCallStageV1::VerifierRecheck)
                || receipt.recheck_decision.is_some()
                || receipt.treatment_output_sha256 != receipt.seeded_candidate_sha256
                || receipt.treatment_disposition.as_deref() != Some("passed_unchanged")
                || receipt.failure_stage.is_some()
                || receipt.failure_code.is_some()
                || receipt.outcome_reason != "verifier_passed_unchanged"
            {
                return Err("unchanged delivery case has repair or output evidence".into());
            }
        }
        Some("needs_revision") => {
            validate_activated_case_terminal(case, receipt)?;
        }
        None => {
            let initial =
                terminal_call_receipt(case, DeliveryVerificationCallStageV1::VerifierInitial);
            let failure_evidence = match (receipt.failure_code.as_deref(), initial) {
                (Some("provider_unavailable" | "timeout"), Some(call)) => {
                    call.status == DeliveryVerificationCallTerminalStatusV1::ProviderFailure
                }
                (Some("invalid_verifier_response"), Some(call)) => {
                    call.status == DeliveryVerificationCallTerminalStatusV1::Completed
                }
                _ => false,
            };
            if !failure_evidence
                || !call_is_unneeded(case, DeliveryVerificationCallStageV1::OwnerRepair)
                || !call_is_unneeded(case, DeliveryVerificationCallStageV1::VerifierRecheck)
                || receipt.treatment_output_sha256.is_some()
                || receipt.treatment_passed != Some(false)
                || !failure_telemetry_is(receipt, "initial_verification")
            {
                return Err(
                    "failed initial verification has inconsistent treatment evidence".into(),
                );
            }
        }
        Some(_) => unreachable!("finding telemetry validator rejects unknown decisions"),
    }
    Ok(())
}

fn validate_activated_case_terminal(
    case: &JournalCaseV1,
    receipt: &CaseTerminalReceiptV1,
) -> Result<(), String> {
    let repair = terminal_call_receipt(case, DeliveryVerificationCallStageV1::OwnerRepair)
        .ok_or_else(|| "activated delivery case lacks OwnerRepair terminal evidence".to_string())?;
    let recheck = terminal_call_receipt(case, DeliveryVerificationCallStageV1::VerifierRecheck);
    if repair.status == DeliveryVerificationCallTerminalStatusV1::Completed {
        if recheck.is_none() {
            return Err("completed delivery repair lacks VerifierRecheck terminal evidence".into());
        }
    } else if !call_is_unneeded(case, DeliveryVerificationCallStageV1::VerifierRecheck) {
        return Err("failed delivery repair has unexpected VerifierRecheck evidence".into());
    }

    match receipt.recheck_decision.as_deref() {
        Some("passed") => {
            let repair = completed_call_receipt(case, DeliveryVerificationCallStageV1::OwnerRepair)
                .ok_or_else(|| "accepted delivery repair lacks OwnerRepair evidence".to_string())?;
            completed_call_receipt(case, DeliveryVerificationCallStageV1::VerifierRecheck)
                .ok_or_else(|| {
                    "accepted delivery repair lacks VerifierRecheck evidence".to_string()
                })?;
            if receipt.treatment_output_sha256.as_ref() != repair.response_artifact_sha256.as_ref()
                || receipt.treatment_disposition.as_deref() != Some("passed_after_repair")
                || receipt.failure_stage.is_some()
                || receipt.failure_code.is_some()
                || receipt.outcome_reason != "repair_accepted"
            {
                return Err("accepted delivery repair has inconsistent output evidence".into());
            }
        }
        Some("needs_revision") => {
            completed_call_receipt(case, DeliveryVerificationCallStageV1::OwnerRepair)
                .ok_or_else(|| "rejected delivery repair lacks OwnerRepair evidence".to_string())?;
            completed_call_receipt(case, DeliveryVerificationCallStageV1::VerifierRecheck)
                .ok_or_else(|| {
                    "rejected delivery repair lacks VerifierRecheck evidence".to_string()
                })?;
            if receipt.treatment_output_sha256.is_some()
                || receipt.treatment_passed != Some(false)
                || receipt.failure_code.as_deref() != Some("verification_not_satisfied")
                || !failure_telemetry_is(receipt, "recheck")
            {
                return Err("rejected delivery repair has inconsistent treatment evidence".into());
            }
        }
        None => {
            let failure_evidence = match receipt.failure_stage.as_deref() {
                Some("owner_repair") => {
                    matches!(
                        receipt.failure_code.as_deref(),
                        Some("provider_unavailable" | "timeout")
                    ) && repair.status == DeliveryVerificationCallTerminalStatusV1::ProviderFailure
                        && recheck.is_none()
                }
                Some("recheck") => match (receipt.failure_code.as_deref(), recheck) {
                    (Some("provider_unavailable" | "timeout"), Some(call)) => {
                        repair.status == DeliveryVerificationCallTerminalStatusV1::Completed
                            && call.status
                                == DeliveryVerificationCallTerminalStatusV1::ProviderFailure
                    }
                    (Some("invalid_verifier_response"), Some(call)) => {
                        repair.status == DeliveryVerificationCallTerminalStatusV1::Completed
                            && call.status == DeliveryVerificationCallTerminalStatusV1::Completed
                    }
                    _ => false,
                },
                _ => false,
            };
            if !failure_evidence
                || receipt.treatment_output_sha256.is_some()
                || receipt.treatment_passed != Some(false)
                || receipt
                    .failure_stage
                    .as_deref()
                    .is_none_or(|stage| !failure_telemetry_is(receipt, stage))
            {
                return Err("failed delivery repair has inconsistent treatment evidence".into());
            }
        }
        Some(_) => unreachable!("finding telemetry validator rejects unknown decisions"),
    }
    Ok(())
}

fn validate_failed_case_terminal(receipt: &CaseTerminalReceiptV1) -> Result<(), String> {
    if receipt.control_passed.is_some()
        || receipt.treatment_passed.is_some()
        || receipt.treatment_output_sha256.is_some()
        || receipt.initial_verifier_decision.is_some()
        || receipt.initial_finding_counts != DeliveryVerificationFindingCounts::default()
        || receipt.repair_activated
        || receipt.recheck_decision.is_some()
        || receipt.recheck_finding_counts != DeliveryVerificationFindingCounts::default()
        || receipt.treatment_disposition.is_some()
        || receipt.failure_stage.is_some()
        || receipt.failure_code.is_some()
    {
        return Err("failed delivery case contains matched-pair or activation claims".into());
    }
    Ok(())
}

fn validate_case_call_sequence(case: &JournalCaseV1, completed_at_ms: u64) -> Result<(), String> {
    let mut prefix_open = true;
    let mut previous_completed = true;
    for call in &case.calls {
        match &call.state {
            JournalCallStateV1::Terminal { receipt, .. } => {
                if !prefix_open || !previous_completed || receipt.terminal_at_ms > completed_at_ms {
                    return Err("delivery case call sequence is invalid".into());
                }
                previous_completed =
                    receipt.status == DeliveryVerificationCallTerminalStatusV1::Completed;
            }
            JournalCallStateV1::Planned | JournalCallStateV1::NotRequired { .. } => {
                prefix_open = false;
            }
            JournalCallStateV1::Reserved { .. } => {
                return Err("delivery case terminal retains a reserved call".into())
            }
        }
    }
    Ok(())
}

fn validate_finding_telemetry(
    decision: Option<&str>,
    counts: DeliveryVerificationFindingCounts,
) -> Result<(), String> {
    let total = counts
        .unsupported_claim
        .checked_add(counts.omitted_obligation)
        .and_then(|value| value.checked_add(counts.contradiction))
        .ok_or_else(|| "delivery finding telemetry overflowed".to_string())?;
    if total > MAX_DELIVERY_VERIFICATION_FINDINGS
        || match decision {
            None | Some("passed") => total != 0,
            Some("needs_revision") => total == 0,
            Some(_) => true,
        }
    {
        return Err("delivery finding telemetry is invalid".into());
    }
    Ok(())
}

fn failure_telemetry_is(receipt: &CaseTerminalReceiptV1, stage: &str) -> bool {
    receipt.treatment_disposition.as_deref() == Some("treatment_failure")
        && receipt.failure_stage.as_deref() == Some(stage)
        && receipt
            .failure_code
            .as_deref()
            .is_some_and(|code| receipt.outcome_reason == format!("component_failure:{code}"))
}

pub(super) fn campaign_deadline_for_reservation(
    reservation: &CampaignReservationReceiptV1,
) -> Result<u64, String> {
    reservation
        .reserved_at_ms
        .checked_add(reservation.max_duration_ms)
        .ok_or_else(|| "delivery campaign deadline overflowed".to_string())
}

pub(super) fn campaign_deadline(document: &JournalDocumentV1) -> Result<u64, String> {
    campaign_deadline_for_reservation(
        document
            .campaign_reservation
            .as_ref()
            .ok_or_else(|| "delivery campaign is not reserved".to_string())?,
    )
}

pub(super) fn campaign_timeout_evidence_sha256() -> String {
    sha256_hex(b"campaign_timeout_exceeded")
}

pub(super) fn validate_campaign_reservation(
    reservation: &CampaignReservationReceiptV1,
    authorization: &DeliveryVerificationAuthorizationV1,
    authorization_consumed_at_ms: u64,
) -> Result<(), String> {
    if reservation.max_logical_model_calls != authorization.budget.max_logical_model_calls_total
        || reservation.max_physical_model_attempts
            != authorization.budget.max_physical_model_attempts_total
        || reservation.max_total_tokens != authorization.budget.max_total_tokens_campaign
        || reservation.max_duration_ms != authorization.budget.campaign_timeout_ms
        || reservation.reserved_at_ms < authorization_consumed_at_ms
        || reservation.reserved_at_ms >= authorization.expires_at_ms
        || reservation.reservation_sha256 != campaign_reservation_digest(reservation)?
    {
        return Err("delivery campaign reservation differs from its authority".into());
    }
    Ok(())
}

pub(super) fn validate_finish_state(
    document: &JournalDocumentV1,
    disposition: DeliveryVerificationCampaignDispositionV1,
) -> Result<(), String> {
    match disposition {
        DeliveryVerificationCampaignDispositionV1::SeededRepairEffective
        | DeliveryVerificationCampaignDispositionV1::NotEffective
        | DeliveryVerificationCampaignDispositionV1::PreservationRegression => {
            let expected_decision = match disposition {
                DeliveryVerificationCampaignDispositionV1::SeededRepairEffective => {
                    "seeded_repair_effective"
                }
                DeliveryVerificationCampaignDispositionV1::NotEffective => "not_effective",
                DeliveryVerificationCampaignDispositionV1::PreservationRegression => {
                    "preservation_regression"
                }
                _ => unreachable!(),
            };
            if document
                .holdout_decision
                .as_ref()
                .is_none_or(|decision| decision.decision != expected_decision)
                || document
                    .cases
                    .iter()
                    .any(|case| !matches!(case.state, JournalCaseStateV1::Terminal { .. }))
            {
                return Err("delivery positive/negative holdout terminal is incomplete".into());
            }
        }
        DeliveryVerificationCampaignDispositionV1::TerminalFutility
            if document
                .calibration_decision
                .as_ref()
                .is_none_or(|decision| decision.decision != "terminal_futility") =>
        {
            return Err("delivery futility terminal lacks calibration decision".into());
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn terminalize(
    document: &mut JournalDocumentV1,
    disposition: DeliveryVerificationCampaignDispositionV1,
    reason: &str,
    evidence_sha256: String,
    terminal_at_ms: u64,
) -> Result<(), String> {
    if document.terminal.is_some() || reason.trim().is_empty() || terminal_at_ms == 0 {
        return Err("delivery campaign is already terminal or lacks terminal data".into());
    }
    require_sha256(&evidence_sha256, "delivery campaign evidence")?;
    let mut terminal = CampaignTerminalReceiptV1 {
        disposition,
        reason: reason.to_string(),
        evidence_sha256,
        terminal_at_ms,
        terminal_sha256: String::new(),
    };
    terminal.terminal_sha256 = terminal_digest(&terminal)?;
    document.phase = JournalPhaseV1::Terminal;
    document.terminal = Some(terminal);
    Ok(())
}

pub(super) fn decision_receipt(
    stage: &str,
    decision: &str,
    counts_sha256: String,
    decided_at_ms: u64,
) -> Result<DecisionReceiptV1, String> {
    require_sha256(&counts_sha256, "delivery matched counts")?;
    if decided_at_ms == 0 {
        return Err("delivery decision timestamp is zero".into());
    }
    let mut receipt = DecisionReceiptV1 {
        stage: stage.into(),
        decision: decision.into(),
        counts_sha256,
        decided_at_ms,
        decision_sha256: String::new(),
    };
    receipt.decision_sha256 = decision_digest(&receipt)?;
    Ok(receipt)
}

pub(super) fn validate_decision(receipt: &DecisionReceiptV1, stage: &str) -> Result<(), String> {
    require_sha256(&receipt.counts_sha256, "delivery matched counts")?;
    if receipt.stage != stage
        || receipt.decision.trim().is_empty()
        || receipt.decided_at_ms == 0
        || receipt.decision_sha256 != decision_digest(receipt)?
    {
        return Err("delivery decision receipt is invalid".into());
    }
    Ok(())
}

pub(super) fn case_resources(
    case: &JournalCaseV1,
) -> Result<DeliveryVerificationObservedResourcesV1, String> {
    let mut resources = DeliveryVerificationObservedResourcesV1::default();
    for call in &case.calls {
        if let JournalCallStateV1::Terminal { receipt, .. } = &call.state {
            accumulate_observed(&mut resources, receipt)?;
        }
    }
    Ok(resources)
}

fn completed_call_receipt(
    case: &JournalCaseV1,
    stage: DeliveryVerificationCallStageV1,
) -> Option<&CallTerminalReceiptV1> {
    match terminal_call_receipt(case, stage) {
        Some(receipt) if receipt.status == DeliveryVerificationCallTerminalStatusV1::Completed => {
            Some(receipt)
        }
        _ => None,
    }
}

fn terminal_call_receipt(
    case: &JournalCaseV1,
    stage: DeliveryVerificationCallStageV1,
) -> Option<&CallTerminalReceiptV1> {
    match &case.calls[stage_index(stage)].state {
        JournalCallStateV1::Terminal { receipt, .. } => Some(receipt),
        _ => None,
    }
}

fn call_is_unneeded(case: &JournalCaseV1, stage: DeliveryVerificationCallStageV1) -> bool {
    matches!(
        &case.calls[stage_index(stage)].state,
        JournalCallStateV1::Planned | JournalCallStateV1::NotRequired { .. }
    )
}

pub(super) fn accumulate_charge(
    charged: &mut DeliveryVerificationChargedResourcesV1,
    reservation: &CallReservationReceiptV1,
) -> Result<(), String> {
    charged.logical_model_calls = checked_add(charged.logical_model_calls, 1, "logical call")?;
    charged.physical_model_attempts =
        checked_add(charged.physical_model_attempts, 1, "physical attempt")?;
    charged.reserved_output_tokens = checked_add(
        charged.reserved_output_tokens,
        reservation.max_output_tokens,
        "reserved output token",
    )?;
    Ok(())
}

pub(super) fn accumulate_observed(
    observed: &mut DeliveryVerificationObservedResourcesV1,
    receipt: &CallTerminalReceiptV1,
) -> Result<(), String> {
    observed.terminal_model_calls = checked_add(observed.terminal_model_calls, 1, "terminal call")?;
    observed.latency_ms = checked_add(observed.latency_ms, receipt.latency_ms, "latency")?;
    if let Some(usage) = &receipt.usage {
        observed.prompt_tokens =
            checked_add(observed.prompt_tokens, usage.prompt_tokens, "prompt token")?;
        observed.completion_tokens = checked_add(
            observed.completion_tokens,
            usage.completion_tokens,
            "completion token",
        )?;
        observed.total_tokens =
            checked_add(observed.total_tokens, usage.total_tokens, "total token")?;
    }
    Ok(())
}

pub(super) fn checked_add(left: u64, right: u64, label: &str) -> Result<u64, String> {
    left.checked_add(right)
        .ok_or_else(|| format!("delivery {label} accounting overflowed"))
}

pub(super) fn require_live(document: &JournalDocumentV1) -> Result<(), String> {
    if !matches!(
        document.phase,
        JournalPhaseV1::CampaignReserved | JournalPhaseV1::Executing
    ) || document.terminal.is_some()
    {
        return Err("delivery execution journal is not live".into());
    }
    Ok(())
}

pub(super) fn case_index(document: &JournalDocumentV1, ordinal: usize) -> Result<usize, String> {
    let index = ordinal
        .checked_sub(1)
        .ok_or_else(|| "delivery case ordinal is invalid".to_string())?;
    if document
        .cases
        .get(index)
        .is_none_or(|case| case.binding.ordinal != ordinal)
    {
        return Err("delivery case ordinal is outside the frozen suite".into());
    }
    Ok(index)
}

pub(super) fn stage_index(stage: DeliveryVerificationCallStageV1) -> usize {
    match stage {
        DeliveryVerificationCallStageV1::VerifierInitial => 0,
        DeliveryVerificationCallStageV1::OwnerRepair => 1,
        DeliveryVerificationCallStageV1::VerifierRecheck => 2,
    }
}

pub(super) fn role_label(role: &ModelRole) -> Result<String, String> {
    match role {
        ModelRole::Executor => Ok("executor".into()),
        ModelRole::Reviewer => Ok("reviewer".into()),
        _ => Err("delivery runner accepts only Executor repair or Reviewer calls".into()),
    }
}

pub(super) struct DeliveryVerificationExecutionLock(File);

impl Drop for DeliveryVerificationExecutionLock {
    fn drop(&mut self) {
        let _ = File::unlock(&self.0);
    }
}

pub(super) fn acquire_execution_lock(
    root: &Path,
) -> Result<DeliveryVerificationExecutionLock, String> {
    let path = root.join(DELIVERY_EXECUTION_LOCK_FILE_NAME);
    let file = loop {
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                validate_lock_metadata(&metadata)?;
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .map_err(|error| format!("failed to open delivery execution lock: {error}"))?;
                break file;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut options = OpenOptions::new();
                options.read(true).write(true).create_new(true);
                #[cfg(unix)]
                options.mode(0o600);
                match options.open(&path) {
                    Ok(file) => break file,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => {
                        return Err(format!("failed to create delivery execution lock: {error}"))
                    }
                }
            }
            Err(error) => {
                return Err(format!(
                    "failed to inspect delivery execution lock: {error}"
                ))
            }
        }
    };
    let path_metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("failed to re-inspect delivery execution lock: {error}"))?;
    validate_lock_metadata(&path_metadata)?;
    let file_metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect opened delivery execution lock: {error}"))?;
    #[cfg(unix)]
    if path_metadata.dev() != file_metadata.dev() || path_metadata.ino() != file_metadata.ino() {
        return Err("delivery execution lock changed while it was opened".into());
    }
    file.try_lock()
        .map_err(|error| format!("delivery execution is already active: {error}"))?;
    Ok(DeliveryVerificationExecutionLock(file))
}

pub(super) fn validate_lock_metadata(metadata: &fs::Metadata) -> Result<(), String> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("delivery execution lock must be a regular non-symlink file".into());
    }
    #[cfg(unix)]
    if metadata.mode() & 0o777 != 0o600 {
        return Err("delivery execution lock permissions must be 0600".into());
    }
    Ok(())
}

pub(super) fn canonical_private_root(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err("delivery output root must be normalized and absolute".into());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect delivery output root: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("delivery output root must be a regular non-symlink directory".into());
    }
    require_private_directory(path, "delivery output root")?;
    path.canonicalize()
        .map_err(|error| format!("failed to canonicalize delivery output root: {error}"))
}

pub(super) fn write_recovery_receipt(
    root: &Path,
    disposition: DeliveryVerificationCampaignDispositionV1,
    reason: &str,
    observed_journal_sha256: Option<String>,
    recovered_at_ms: u64,
) -> Result<RecoveryReceiptV1, String> {
    if let Some(value) = &observed_journal_sha256 {
        require_sha256(value, "delivery observed journal")?;
    }
    if reason.trim().is_empty() || recovered_at_ms == 0 {
        return Err("delivery recovery receipt is incomplete".into());
    }
    let mut receipt = RecoveryReceiptV1 {
        schema: DELIVERY_EXECUTION_RECOVERY_SCHEMA.into(),
        disposition,
        reason: reason.into(),
        output_root_sha256: path_sha256(root),
        observed_journal_sha256,
        recovered_at_ms,
        recovery_sha256: String::new(),
    };
    receipt.recovery_sha256 = recovery_digest(&receipt)?;
    write_new_private_file(
        &root.join(DELIVERY_EXECUTION_RECOVERY_FILE_NAME),
        &canonical_json(&receipt, "delivery execution recovery")?,
        "delivery execution recovery",
    )?;
    Ok(receipt)
}

pub(super) fn read_recovery_receipt(root: &Path) -> Result<RecoveryReceiptV1, String> {
    let bytes = read_private_file(
        &root.join(DELIVERY_EXECUTION_RECOVERY_FILE_NAME),
        "delivery execution recovery",
    )?;
    let receipt: RecoveryReceiptV1 = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid delivery recovery JSON: {error}"))?;
    if canonical_json(&receipt, "delivery execution recovery")? != bytes
        || receipt.schema != DELIVERY_EXECUTION_RECOVERY_SCHEMA
        || receipt.output_root_sha256 != path_sha256(root)
        || receipt.reason.trim().is_empty()
        || receipt.recovered_at_ms == 0
        || receipt.recovery_sha256 != recovery_digest(&receipt)?
    {
        return Err("delivery execution recovery receipt is invalid".into());
    }
    Ok(receipt)
}

pub(super) fn campaign_reservation_digest(
    value: &CampaignReservationReceiptV1,
) -> Result<String, String> {
    let mut payload = value.clone();
    payload.reservation_sha256.clear();
    domain_digest(
        CAMPAIGN_RESERVATION_HASH_DOMAIN,
        &payload,
        "delivery campaign reservation",
    )
}

pub(super) fn call_reservation_digest(value: &CallReservationReceiptV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.reservation_sha256.clear();
    domain_digest(
        CALL_RESERVATION_HASH_DOMAIN,
        &payload,
        "delivery call reservation",
    )
}

pub(super) fn call_terminal_digest(value: &CallTerminalReceiptV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.terminal_sha256.clear();
    domain_digest(CALL_TERMINAL_HASH_DOMAIN, &payload, "delivery call receipt")
}

pub(super) fn case_terminal_digest(value: &CaseTerminalReceiptV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.receipt_sha256.clear();
    domain_digest(CASE_TERMINAL_HASH_DOMAIN, &payload, "delivery case receipt")
}

pub(super) fn decision_digest(value: &DecisionReceiptV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.decision_sha256.clear();
    domain_digest(DECISION_HASH_DOMAIN, &payload, "delivery decision receipt")
}

pub(super) fn terminal_digest(value: &CampaignTerminalReceiptV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.terminal_sha256.clear();
    domain_digest(TERMINAL_HASH_DOMAIN, &payload, "delivery campaign terminal")
}

pub(super) fn journal_digest(value: &JournalDocumentV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.journal_sha256.clear();
    domain_digest(JOURNAL_HASH_DOMAIN, &payload, "delivery execution journal")
}

pub(super) fn recovery_digest(value: &RecoveryReceiptV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.recovery_sha256.clear();
    domain_digest(
        RECOVERY_HASH_DOMAIN,
        &payload,
        "delivery execution recovery",
    )
}
