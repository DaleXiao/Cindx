use super::*;
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
        || input.canonical_request_bytes == 0
        || input.canonical_request_bytes > MAX_CANONICAL_REQUEST_BYTES
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
    require_sha256(
        &input.canonical_request_sha256,
        "delivery canonical request",
    )?;
    let (expected_role, expected_model, expected_output) = match input.stage {
        DeliveryVerificationCallStageV1::OwnerDraft
        | DeliveryVerificationCallStageV1::OwnerRepair => (
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
        || reservation.canonical_request_bytes == 0
        || reservation.canonical_request_bytes > MAX_CANONICAL_REQUEST_BYTES
        || reservation.reservation_sha256 != call_reservation_digest(reservation)?
    {
        return Err("delivery stored call reservation is invalid".into());
    }
    require_sha256(
        &reservation.configured_model_sha256,
        "delivery configured model",
    )?;
    require_sha256(
        &reservation.canonical_request_sha256,
        "delivery canonical request",
    )?;
    let (role, model, output) = match reservation.stage {
        DeliveryVerificationCallStageV1::OwnerDraft
        | DeliveryVerificationCallStageV1::OwnerRepair => (
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
                || receipt.request_payload_sha256.as_ref()
                    != Some(&reservation.canonical_request_sha256)
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

pub(super) fn validate_case_terminal(
    case: &JournalCaseV1,
    receipt: &CaseTerminalReceiptV1,
) -> Result<(), String> {
    require_sha256(&receipt.observation_sha256, "delivery case observation")?;
    if receipt.completed_at_ms == 0 || receipt.receipt_sha256 != case_terminal_digest(receipt)? {
        return Err("delivery case terminal receipt is invalid".into());
    }
    for digest in [
        receipt.owner_draft_sha256.as_deref(),
        receipt.control_output_sha256.as_deref(),
        receipt.treatment_output_sha256.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        require_sha256(digest, "delivery case output")?;
    }
    if receipt.owner_draft_sha256.is_some() != receipt.owner_draft_bytes.is_some()
        || receipt.owner_draft_bytes == Some(0)
        || receipt.control_output_sha256.is_some()
            && receipt.control_output_sha256 != receipt.owner_draft_sha256
        || receipt.resources != case_resources(case)?
    {
        return Err("delivery case output or resource receipt is inconsistent".into());
    }
    if receipt.status == DeliveryVerificationCaseTerminalStatusV1::Complete {
        let owner_draft = completed_call_receipt(case, DeliveryVerificationCallStageV1::OwnerDraft)
            .ok_or_else(|| "complete delivery case lacks OwnerDraft evidence".to_string())?;
        completed_call_receipt(case, DeliveryVerificationCallStageV1::VerifierInitial)
            .ok_or_else(|| "complete delivery case lacks VerifierInitial evidence".to_string())?;
        let treatment = match (
            completed_call_receipt(case, DeliveryVerificationCallStageV1::OwnerRepair),
            completed_call_receipt(case, DeliveryVerificationCallStageV1::VerifierRecheck),
        ) {
            (Some(owner_repair), Some(_)) => owner_repair,
            (None, None)
                if call_is_unneeded(case, DeliveryVerificationCallStageV1::OwnerRepair)
                    && call_is_unneeded(case, DeliveryVerificationCallStageV1::VerifierRecheck) =>
            {
                owner_draft
            }
            _ => {
                return Err(
                    "complete delivery case has unmatched repair and verifier evidence".into(),
                )
            }
        };
        if receipt.control_passed.is_none()
            || receipt.treatment_passed.is_none()
            || receipt.owner_draft_sha256.as_ref() != owner_draft.response_artifact_sha256.as_ref()
            || receipt.owner_draft_bytes != owner_draft.response_artifact_bytes
            || receipt.control_output_sha256.as_ref()
                != owner_draft.response_artifact_sha256.as_ref()
            || receipt.treatment_output_sha256.as_ref()
                != treatment.response_artifact_sha256.as_ref()
        {
            return Err("complete delivery case output digests do not match call artifacts".into());
        }
    }
    Ok(())
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
        DeliveryVerificationCampaignDispositionV1::EvidenceOfUplift
        | DeliveryVerificationCampaignDispositionV1::NoEvidence
        | DeliveryVerificationCampaignDispositionV1::Regression => {
            let expected_decision = match disposition {
                DeliveryVerificationCampaignDispositionV1::EvidenceOfUplift => "evidence_of_uplift",
                DeliveryVerificationCampaignDispositionV1::NoEvidence => "no_evidence",
                DeliveryVerificationCampaignDispositionV1::Regression => "regression",
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
    match &case.calls[stage_index(stage)].state {
        JournalCallStateV1::Terminal { receipt, .. }
            if receipt.status == DeliveryVerificationCallTerminalStatusV1::Completed =>
        {
            Some(receipt)
        }
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
        DeliveryVerificationCallStageV1::OwnerDraft => 0,
        DeliveryVerificationCallStageV1::VerifierInitial => 1,
        DeliveryVerificationCallStageV1::OwnerRepair => 2,
        DeliveryVerificationCallStageV1::VerifierRecheck => 3,
    }
}

pub(super) fn role_label(role: &ModelRole) -> Result<String, String> {
    match role {
        ModelRole::Executor => Ok("executor".into()),
        ModelRole::Reviewer => Ok("reviewer".into()),
        _ => Err("delivery runner accepts only Executor Owner or Reviewer calls".into()),
    }
}

pub(super) fn acquire_execution_lock(root: &Path) -> Result<File, String> {
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
    Ok(file)
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
