//! Records the deterministic delivery-verification state for a committed answer
//! (P1-10).
//!
//! The contract itself — subject binding, the typed verdict, and the
//! one-repair-one-recheck state machine — lives in `agent-runtime`. This module is
//! the product seam: it builds the reference context from the run's own outcome
//! ledger and contract evidence, binds the exact answer bytes, produces the
//! verdict from the claim-evidence receipt, drives the state to a terminal status,
//! and records that state on the terminal metadata beside the grounded-completion
//! receipt and the outcome ledger.
//!
//! It is additive and fail-open by construction. Any binding failure records an
//! `unbound` status with its reason instead of failing a commit, because this
//! record is verification evidence and never a delivery decision: the judge gate
//! still owns repair and fail-closed, and this state changes nothing about what
//! the user receives.

use agent_core::Metadata;
use agent_runtime::{
    bind_answer_citations, observed_locations_from_messages, verify_delivery_against_claims,
    DeliveryVerificationEvidence, DeliveryVerificationObligation, GroundedCompletionReceipt,
    OutcomeLedgerShadow,
};

pub(crate) const DELIVERY_VERIFICATION_RECORD_SCHEMA: &str = "cindx.agent.delivery-verification.v1";
/// Recorded when the subject could not be bound, so a missing verification is
/// never mistaken for a passing one.
const UNBOUND_STATUS: &str = "unbound";

pub(crate) fn record_delivery_verification_state(
    runtime: &agent_runtime::AgentLoopState,
    objective: &str,
    final_answer: &str,
    receipt: &GroundedCompletionReceipt,
    ledger: &OutcomeLedgerShadow,
    metadata: &mut Metadata,
) {
    metadata.insert(
        "delivery_verification_schema".to_string(),
        DELIVERY_VERIFICATION_RECORD_SCHEMA.to_string(),
    );
    let claims = bind_answer_citations(
        final_answer,
        &observed_locations_from_messages(&runtime.messages),
    );
    let (obligations, evidence) = delivery_verification_reference_context(runtime, receipt, ledger);
    match verify_delivery_against_claims(
        objective,
        final_answer,
        receipt,
        &obligations,
        &evidence,
        &claims,
    ) {
        Ok(state) => {
            metadata.insert(
                "delivery_verification_status".to_string(),
                state.status().label().to_string(),
            );
            metadata.insert(
                "delivery_verification_subject_sha256".to_string(),
                state.subject().subject_sha256.clone(),
            );
            metadata.insert(
                "delivery_verification_answer_sha256".to_string(),
                claims.answer_sha256.clone(),
            );
            metadata.insert(
                "delivery_verification_citations".to_string(),
                claims.citations_checked.to_string(),
            );
            if let Some(verdict) = state.initial_verdict() {
                metadata.insert(
                    "delivery_verification_verdict_sha256".to_string(),
                    verdict.receipt_sha256.clone(),
                );
                metadata.insert(
                    "delivery_verification_findings".to_string(),
                    verdict.findings.len().to_string(),
                );
            }
        }
        Err(issue) => {
            metadata.insert(
                "delivery_verification_status".to_string(),
                UNBOUND_STATUS.to_string(),
            );
            metadata.insert("delivery_verification_issue".to_string(), issue.to_string());
        }
    }
}

/// The reference context the subject binds: one entry per obligation the grounded
/// receipt covered, in the receipt's order, and one per evidence sequence the
/// model could see.
///
/// The content is each reference's typed identity — the obligation's kind and
/// satisfaction, the evidence's kind and source tool — and never invented prose.
/// The deterministic verdict producer does not read this text, but the subject
/// digest binds it, so a future provider-backed producer cannot be handed a
/// different reference context than the one recorded here.
fn delivery_verification_reference_context(
    runtime: &agent_runtime::AgentLoopState,
    receipt: &GroundedCompletionReceipt,
    ledger: &OutcomeLedgerShadow,
) -> (
    Vec<DeliveryVerificationObligation>,
    Vec<DeliveryVerificationEvidence>,
) {
    let obligations = receipt
        .covered_obligation_ids
        .iter()
        .filter_map(|obligation_id| {
            let obligation = ledger
                .obligations
                .iter()
                .find(|item| item.id == *obligation_id)?;
            Some(DeliveryVerificationObligation {
                obligation_ref: obligation_id.clone(),
                content: format!(
                    "obligation {:?} {:?}",
                    obligation.kind, obligation.satisfaction
                ),
            })
        })
        .collect();
    let evidence = receipt
        .visible_evidence_sequences
        .iter()
        .filter_map(|sequence| {
            let entry = runtime
                .task_contract
                .evidence()
                .iter()
                .find(|item| item.sequence == *sequence)?;
            Some(DeliveryVerificationEvidence {
                evidence_ref: *sequence,
                content: format!("evidence {:?} from {}", entry.kind, entry.source),
            })
        })
        .collect();
    (obligations, evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::start_agent_loop;

    /// The seam must never turn a verification problem into a delivery failure:
    /// with a receipt that does not bind the candidate, it records `unbound` plus
    /// the reason and leaves the rest of the metadata alone.
    #[test]
    fn an_unbindable_subject_is_recorded_as_unbound_and_never_fails_the_commit() {
        let runtime = start_agent_loop(
            agent_core::TaskId("delivery-verification-unbound".to_string()),
            "objective",
            agent_runtime::AgentRuntimeConfig::default(),
        );
        // A receipt for different answer bytes than the one being committed.
        let receipt = agent_runtime::GroundedCompletionReceipt {
            schema: agent_runtime::GROUNDED_COMPLETION_SCHEMA.to_string(),
            steer_epoch: 0,
            contract_epoch: 0,
            model_turn: 1,
            content_sha256: "a".repeat(64),
            content_bytes: 999,
            obligation_digest: "b".repeat(64),
            covered_obligation_ids: Vec::new(),
            visible_evidence_sequences: Vec::new(),
            constraint_codes: Vec::new(),
            basis: agent_runtime::GroundedCompletionBasis::EvidenceVisible,
        };
        let ledger = OutcomeLedgerShadow {
            schema: agent_runtime::OUTCOME_LEDGER_SCHEMA.to_string(),
            steer_epoch: 0,
            phase: agent_runtime::OutcomeLedgerPhase::Completed,
            obligations: Vec::new(),
            evidence: Vec::new(),
            claims: Vec::new(),
            postconditions: Vec::new(),
            terminal: None,
            failure: None,
            truncation: agent_runtime::OutcomeTruncation::default(),
        };
        let mut metadata = Metadata::new();

        record_delivery_verification_state(
            &runtime,
            "objective",
            "The committed answer cites src/x.rs:3.",
            &receipt,
            &ledger,
            &mut metadata,
        );

        assert_eq!(
            metadata
                .get("delivery_verification_schema")
                .map(String::as_str),
            Some(DELIVERY_VERIFICATION_RECORD_SCHEMA)
        );
        assert_eq!(
            metadata
                .get("delivery_verification_status")
                .map(String::as_str),
            Some(UNBOUND_STATUS)
        );
        assert!(
            metadata
                .get("delivery_verification_issue")
                .is_some_and(|issue| !issue.is_empty()),
            "the reason is recorded, not swallowed"
        );
        // No verdict or subject digest is claimed for a state that never bound.
        assert!(
            !metadata.contains_key("delivery_verification_verdict_sha256"),
            "an unbound state must not report a verdict digest"
        );
    }
}
