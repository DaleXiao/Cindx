use super::{
    fingerprint, AgentActionDenialKind, AgentTaskContract, ContractEvidenceKind,
    OutcomeLedgerPhase, OutcomeLedgerShadow, OutcomeObligationKind, OutcomePostcondition,
    OutcomePostconditionStatus, OutcomeSatisfaction, OutcomeScope,
};
use agent_core::Metadata;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const GROUNDED_COMPLETION_SCHEMA: &str = "cindx.agent.grounded-completion.v1";
pub const GROUNDED_COMPLETION_METADATA_KEY: &str = "grounded_completion_v1";
pub const GROUNDED_COMPLETION_DIGEST_METADATA_KEY: &str = "grounded_completion_digest";

const MAX_COVERED_REQUIREMENTS: usize = 20;
const MAX_VISIBLE_EVIDENCE: usize = 16;
const MAX_METADATA_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundedCompletionBasis {
    SelfContained,
    EvidenceVisible,
    PostconditionVerified,
    ConstraintObserved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroundedCompletionReceipt {
    pub schema: String,
    pub steer_epoch: u64,
    pub contract_epoch: u64,
    pub model_turn: u64,
    pub content_sha256: String,
    pub content_bytes: u64,
    pub obligation_digest: String,
    pub covered_obligation_ids: Vec<String>,
    pub visible_evidence_sequences: Vec<u64>,
    #[serde(default)]
    pub constraint_codes: Vec<String>,
    pub basis: GroundedCompletionBasis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroundedCompletionIssue {
    EmptyAnswer,
    InvalidLedger,
    TruncatedLedger,
    PendingObligations(Vec<String>),
    PendingPostconditions(Vec<String>),
    MissingEvidence(Vec<String>),
    EvidenceNotVisible(Vec<u64>),
    MissingConstraintDisclosure(Vec<String>),
    InvalidReceipt,
    ReceiptLineageChanged,
}

impl GroundedCompletionReceipt {
    pub fn insert_metadata(&self, metadata: &mut Metadata) -> bool {
        let Some((encoded, digest)) = self.metadata_values() else {
            return false;
        };
        metadata.insert(GROUNDED_COMPLETION_METADATA_KEY.to_string(), encoded);
        metadata.insert(GROUNDED_COMPLETION_DIGEST_METADATA_KEY.to_string(), digest);
        true
    }

    pub fn from_metadata(metadata: &Metadata) -> Option<Self> {
        let encoded = metadata.get(GROUNDED_COMPLETION_METADATA_KEY)?;
        let expected_digest = metadata.get(GROUNDED_COMPLETION_DIGEST_METADATA_KEY)?;
        if encoded.len() > MAX_METADATA_BYTES
            || !is_sha256_hex(expected_digest)
            || fingerprint(encoded) != *expected_digest
        {
            return None;
        }
        let receipt = serde_json::from_str::<Self>(encoded).ok()?;
        if metadata.get("steer_epoch")?.parse::<u64>().ok()? != receipt.steer_epoch {
            return None;
        }
        receipt.contract_is_valid().then_some(receipt)
    }

    pub fn from_terminal_metadata(metadata: &Metadata, answer: &str) -> Option<Self> {
        let receipt = Self::from_metadata(metadata)?;
        if receipt.content_sha256 != fingerprint(answer)
            || receipt.content_bytes != answer.len() as u64
        {
            return None;
        }
        let ledger =
            OutcomeLedgerShadow::from_terminal_metadata(metadata, OutcomeLedgerPhase::Completed)?;
        if ledger.steer_epoch != receipt.steer_epoch
            || ledger.truncation.obligations > 0
            || ledger
                .obligations
                .iter()
                .any(|item| item.satisfaction == OutcomeSatisfaction::Pending)
            || ledger
                .postconditions
                .iter()
                .any(|item| item.required && item.status == OutcomePostconditionStatus::Pending)
            || !obligation_evidence_kinds_are_valid(&ledger)
        {
            return None;
        }
        if contract_epoch_for_ledger(&ledger, receipt.steer_epoch)? != receipt.contract_epoch {
            return None;
        }
        let terminal = ledger.terminal.as_ref()?;
        let delivered = ledger.claims.iter().find(|claim| {
            claim.sequence == terminal.delivered_claim_sequence
                && claim.kind == super::OutcomeClaimKind::DeliveredAnswer
        })?;
        let required_postconditions = required_postconditions(&ledger);
        let covered_ids = covered_requirement_ids(&ledger, &required_postconditions);
        let required_evidence = required_evidence_sequences(&ledger)
            .into_iter()
            .collect::<Vec<_>>();
        let expected_constraint_codes = constraint_codes(&ledger);
        let expected_basis = if !expected_constraint_codes.is_empty() {
            GroundedCompletionBasis::ConstraintObserved
        } else if required_postconditions
            .iter()
            .any(|item| item.status == OutcomePostconditionStatus::Verified)
        {
            GroundedCompletionBasis::PostconditionVerified
        } else if covered_ids.is_empty() {
            GroundedCompletionBasis::SelfContained
        } else {
            GroundedCompletionBasis::EvidenceVisible
        };
        if delivered.steer_epoch != receipt.steer_epoch
            || delivered.model_turn != receipt.model_turn
            || delivered.content_sha256 != receipt.content_sha256
            || delivered.content_bytes != receipt.content_bytes
            || delivered.available_evidence_sequences != required_evidence
            || receipt.visible_evidence_sequences != required_evidence
            || receipt.constraint_codes != expected_constraint_codes
            || receipt.covered_obligation_ids != covered_ids
            || receipt.basis != expected_basis
            || receipt.obligation_digest
                != obligation_digest(&ledger, &required_postconditions).ok()?
        {
            return None;
        }
        Some(receipt)
    }

    pub fn contract_is_valid(&self) -> bool {
        self.semantic_contract_is_valid()
            && serde_json::to_vec(self).is_ok_and(|encoded| encoded.len() <= MAX_METADATA_BYTES)
    }

    fn metadata_values(&self) -> Option<(String, String)> {
        if !self.semantic_contract_is_valid() {
            return None;
        }
        let encoded = serde_json::to_string(self).ok()?;
        (encoded.len() <= MAX_METADATA_BYTES).then(|| (encoded.clone(), fingerprint(&encoded)))
    }

    fn semantic_contract_is_valid(&self) -> bool {
        self.schema == GROUNDED_COMPLETION_SCHEMA
            && self.content_bytes > 0
            && self.contract_epoch <= self.steer_epoch
            && is_sha256_hex(&self.content_sha256)
            && is_sha256_hex(&self.obligation_digest)
            && self.covered_obligation_ids.len() <= MAX_COVERED_REQUIREMENTS
            && self.visible_evidence_sequences.len() <= MAX_VISIBLE_EVIDENCE
            && strictly_increasing(self.visible_evidence_sequences.iter().copied())
            && strictly_increasing_strings(&self.covered_obligation_ids)
            && self
                .covered_obligation_ids
                .iter()
                .all(|id| is_sha256_hex(id))
            && match self.basis {
                GroundedCompletionBasis::SelfContained => {
                    self.covered_obligation_ids.is_empty()
                        && self.visible_evidence_sequences.is_empty()
                        && self.constraint_codes.is_empty()
                }
                GroundedCompletionBasis::EvidenceVisible
                | GroundedCompletionBasis::PostconditionVerified => {
                    !self.covered_obligation_ids.is_empty()
                        && !self.visible_evidence_sequences.is_empty()
                        && self.constraint_codes.is_empty()
                }
                GroundedCompletionBasis::ConstraintObserved => {
                    !self.covered_obligation_ids.is_empty()
                        && !self.visible_evidence_sequences.is_empty()
                        && !self.constraint_codes.is_empty()
                        && self.constraint_codes.len() <= MAX_COVERED_REQUIREMENTS
                        && strictly_increasing_strings(&self.constraint_codes)
                }
            }
    }
}

impl AgentTaskContract {
    pub fn grounded_completion_receipt(
        &self,
        steer_epoch: u64,
        model_turn: usize,
        answer: &str,
        visible_evidence_sequences: &[u64],
    ) -> Result<GroundedCompletionReceipt, GroundedCompletionIssue> {
        if answer.trim().is_empty() {
            return Err(GroundedCompletionIssue::EmptyAnswer);
        }
        let ledger = self.outcome_ledger_shadow(steer_epoch);
        if !ledger.contract_is_valid() {
            return Err(GroundedCompletionIssue::InvalidLedger);
        }
        let contract_epoch = contract_epoch_for_ledger(&ledger, steer_epoch)
            .ok_or(GroundedCompletionIssue::InvalidLedger)?;
        // Postcondition projection retains the newest pending item for each
        // bounded surface; older truncated entries are historical verified or
        // superseded steps. Obligation truncation can still hide live work.
        if ledger.truncation.obligations > 0 {
            return Err(GroundedCompletionIssue::TruncatedLedger);
        }

        let pending_postconditions = ledger
            .postconditions
            .iter()
            .filter(|item| item.required && item.status == OutcomePostconditionStatus::Pending)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        if !pending_postconditions.is_empty() {
            return Err(GroundedCompletionIssue::PendingPostconditions(
                pending_postconditions,
            ));
        }
        let pending_obligations = ledger
            .obligations
            .iter()
            .filter(|item| item.satisfaction == OutcomeSatisfaction::Pending)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        if !pending_obligations.is_empty() {
            return Err(GroundedCompletionIssue::PendingObligations(
                pending_obligations,
            ));
        }

        let constraint_codes = constraint_codes(&ledger);
        if !constraint_codes.is_empty() && !constraint_disclosed(answer, &ledger) {
            return Err(GroundedCompletionIssue::MissingConstraintDisclosure(
                constraint_codes,
            ));
        }

        let visible_evidence = visible_evidence_sequences
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        let missing_evidence = ledger
            .obligations
            .iter()
            .filter(|item| item.evidence_sequence.is_none())
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        if !missing_evidence.is_empty() {
            return Err(GroundedCompletionIssue::MissingEvidence(missing_evidence));
        }

        let required_evidence = required_evidence_sequences(&ledger);
        let not_visible = required_evidence
            .difference(&visible_evidence)
            .copied()
            .collect::<Vec<_>>();
        if !not_visible.is_empty() {
            return Err(GroundedCompletionIssue::EvidenceNotVisible(not_visible));
        }

        let required_postconditions = required_postconditions(&ledger);
        let covered_obligation_ids = covered_requirement_ids(&ledger, &required_postconditions);
        let verified_postcondition = required_postconditions
            .iter()
            .any(|item| item.status == OutcomePostconditionStatus::Verified);
        let basis = if !constraint_codes.is_empty() {
            GroundedCompletionBasis::ConstraintObserved
        } else if verified_postcondition {
            GroundedCompletionBasis::PostconditionVerified
        } else if covered_obligation_ids.is_empty() {
            GroundedCompletionBasis::SelfContained
        } else {
            GroundedCompletionBasis::EvidenceVisible
        };
        let obligation_digest = obligation_digest(&ledger, &required_postconditions)
            .map_err(|_| GroundedCompletionIssue::InvalidLedger)?;
        Ok(GroundedCompletionReceipt {
            schema: GROUNDED_COMPLETION_SCHEMA.to_string(),
            steer_epoch,
            contract_epoch,
            model_turn: model_turn as u64,
            content_sha256: fingerprint(answer),
            content_bytes: answer.len() as u64,
            obligation_digest,
            covered_obligation_ids,
            visible_evidence_sequences: required_evidence.into_iter().collect(),
            constraint_codes,
            basis,
        })
    }

    pub fn grounded_completion_required_evidence_sequences(&self, steer_epoch: u64) -> Vec<u64> {
        required_evidence_sequences(&self.outcome_ledger_shadow(steer_epoch))
            .into_iter()
            .collect()
    }

    pub fn rebind_grounded_completion_receipt(
        &self,
        previous: &GroundedCompletionReceipt,
        steer_epoch: u64,
        model_turn: usize,
        final_answer: &str,
        visible_evidence_sequences: &[u64],
    ) -> Result<GroundedCompletionReceipt, GroundedCompletionIssue> {
        if !previous.contract_is_valid() {
            return Err(GroundedCompletionIssue::InvalidReceipt);
        }
        let rebound = self.grounded_completion_receipt(
            steer_epoch,
            model_turn,
            final_answer,
            visible_evidence_sequences,
        )?;
        if previous.steer_epoch != rebound.steer_epoch
            || previous.contract_epoch != rebound.contract_epoch
            || previous.obligation_digest != rebound.obligation_digest
            || previous.covered_obligation_ids != rebound.covered_obligation_ids
            || previous.visible_evidence_sequences != rebound.visible_evidence_sequences
            || previous.basis != rebound.basis
        {
            return Err(GroundedCompletionIssue::ReceiptLineageChanged);
        }
        Ok(rebound)
    }
}

fn contract_epoch_for_ledger(ledger: &OutcomeLedgerShadow, execution_epoch: u64) -> Option<u64> {
    let epochs = ledger
        .obligations
        .iter()
        .filter(|item| item.scope == OutcomeScope::Steer)
        .map(|item| item.steer_epoch)
        .collect::<Option<BTreeSet<_>>>()?;
    match epochs.len() {
        0 => Some(execution_epoch),
        1 => epochs
            .into_iter()
            .next()
            .filter(|contract_epoch| *contract_epoch <= execution_epoch),
        _ => None,
    }
}

fn required_evidence_sequences(ledger: &OutcomeLedgerShadow) -> BTreeSet<u64> {
    ledger
        .obligations
        .iter()
        .filter_map(|item| item.evidence_sequence)
        .chain(
            ledger
                .postconditions
                .iter()
                .filter(|item| item.required && item.status == OutcomePostconditionStatus::Verified)
                .flat_map(|item| {
                    std::iter::once(item.action_sequence).chain(item.observation_sequence)
                }),
        )
        .collect()
}

fn required_postconditions(ledger: &OutcomeLedgerShadow) -> Vec<&OutcomePostcondition> {
    ledger
        .postconditions
        .iter()
        .filter(|item| item.required && item.status != OutcomePostconditionStatus::Superseded)
        .collect()
}

fn covered_requirement_ids(
    ledger: &OutcomeLedgerShadow,
    postconditions: &[&OutcomePostcondition],
) -> Vec<String> {
    ledger
        .obligations
        .iter()
        .map(|item| item.id.clone())
        .chain(postconditions.iter().map(|item| item.id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn obligation_digest(
    ledger: &OutcomeLedgerShadow,
    postconditions: &[&OutcomePostcondition],
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&(&ledger.obligations, postconditions))
        .map(|encoded| fingerprint(&encoded))
}

fn obligation_evidence_kinds_are_valid(ledger: &OutcomeLedgerShadow) -> bool {
    let evidence = ledger
        .evidence
        .iter()
        .map(|item| (item.sequence, item.kind))
        .collect::<std::collections::BTreeMap<_, _>>();
    ledger.obligations.iter().all(|item| {
        let Some(sequence) = item.evidence_sequence else {
            return false;
        };
        if item.satisfaction == OutcomeSatisfaction::Blocked {
            return item.blocker.is_some()
                && matches!(evidence.get(&sequence), Some(ContractEvidenceKind::Denial));
        }
        item.satisfaction == OutcomeSatisfaction::Satisfied
            && matches!(
                (item.kind, evidence.get(&sequence)),
                (
                    OutcomeObligationKind::RequiredTool,
                    Some(ContractEvidenceKind::RequiredTool)
                ) | (
                    OutcomeObligationKind::Grounding,
                    Some(ContractEvidenceKind::Grounding)
                ) | (
                    OutcomeObligationKind::WorkspaceVerification,
                    Some(ContractEvidenceKind::Verification),
                ) | (
                    OutcomeObligationKind::InteractionObservation,
                    Some(ContractEvidenceKind::InteractionObservation),
                ) | (OutcomeObligationKind::AnyTool, Some(_))
            )
    })
}

fn constraint_codes(ledger: &OutcomeLedgerShadow) -> Vec<String> {
    ledger
        .obligations
        .iter()
        .filter(|item| item.satisfaction == OutcomeSatisfaction::Blocked)
        .filter_map(|item| item.blocker.as_ref().map(|blocker| blocker.code.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn constraint_disclosed(answer: &str, ledger: &OutcomeLedgerShadow) -> bool {
    let normalized = answer.to_ascii_lowercase();
    ledger
        .obligations
        .iter()
        .filter(|item| item.satisfaction == OutcomeSatisfaction::Blocked)
        .filter_map(|item| item.blocker.as_ref())
        .all(|blocker| {
            normalized.contains(&blocker.code)
                || match blocker.kind {
                    AgentActionDenialKind::UserPermission => {
                        (normalized.contains("permission") && normalized.contains("denied"))
                            || (answer.contains("权限") && answer.contains("拒绝"))
                    }
                    AgentActionDenialKind::RuntimePolicy => {
                        (normalized.contains("policy")
                            && (normalized.contains("blocked") || normalized.contains("denied")))
                            || (answer.contains("策略")
                                && (answer.contains("阻止") || answer.contains("拒绝")))
                    }
                    AgentActionDenialKind::CapabilityUnavailable => {
                        normalized.contains("unavailable") || answer.contains("不可用")
                    }
                    AgentActionDenialKind::RepeatedAction => {
                        (normalized.contains("repeated") && normalized.contains("blocked"))
                            || (answer.contains("重复") && answer.contains("阻止"))
                    }
                }
        })
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn strictly_increasing(values: impl IntoIterator<Item = u64>) -> bool {
    let mut previous = None;
    values.into_iter().all(|value| {
        let valid = previous.is_none_or(|previous| previous < value);
        previous = Some(value);
        valid
    })
}

fn strictly_increasing_strings(values: &[String]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        OutcomeClaimDecision, OutcomeClaimEvidenceStatus, OutcomeClaimKind,
        OutcomeTerminalObservation, ResultQuality, WorkspaceVerificationPolicy,
    };
    use agent_core::{ToolOutcomeStatus, ToolRisk};

    fn completed_metadata(
        contract: &AgentTaskContract,
        receipt: &GroundedCompletionReceipt,
        answer: &str,
    ) -> Metadata {
        let ledger = contract.completed_outcome_ledger(OutcomeTerminalObservation {
            steer_epoch: receipt.steer_epoch,
            model_turn: receipt.model_turn as usize,
            answer,
            selected_stage: "executor",
            selector_quality: match receipt.basis {
                GroundedCompletionBasis::SelfContained => ResultQuality::Substantive,
                GroundedCompletionBasis::EvidenceVisible => ResultQuality::Grounded,
                GroundedCompletionBasis::PostconditionVerified => ResultQuality::Verified,
                GroundedCompletionBasis::ConstraintObserved => ResultQuality::Substantive,
            },
            selector_marked_verified: receipt.basis
                == GroundedCompletionBasis::PostconditionVerified,
            selector_marked_deliverable: true,
            selector_evidence_count: receipt.visible_evidence_sequences.len(),
            trusted_evidence_sequences: &receipt.visible_evidence_sequences,
        });
        let mut metadata = Metadata::from([
            ("steer_epoch".to_string(), receipt.steer_epoch.to_string()),
            ("outcome_ledger_status".to_string(), "recorded".to_string()),
        ]);
        assert!(receipt.insert_metadata(&mut metadata));
        assert!(ledger.insert_metadata(&mut metadata));
        metadata
    }

    #[test]
    fn self_contained_receipt_binds_exact_bytes_and_round_trips() {
        let contract = AgentTaskContract::default();
        let receipt = contract
            .grounded_completion_receipt(3, 4, " answer ", &[])
            .expect("self-contained answer should complete");
        assert_eq!(receipt.basis, GroundedCompletionBasis::SelfContained);
        assert_eq!(receipt.content_sha256, fingerprint(" answer "));
        assert_eq!(receipt.content_bytes, 8);

        let mut metadata = Metadata::new();
        metadata.insert("steer_epoch".to_string(), "3".to_string());
        assert!(receipt.insert_metadata(&mut metadata));
        assert_eq!(
            GroundedCompletionReceipt::from_metadata(&metadata),
            Some(receipt)
        );
    }

    #[test]
    fn pending_and_truncated_ledgers_fail_closed() {
        let mut pending = AgentTaskContract::default();
        pending.require_tool_success("file.read");
        assert!(matches!(
            pending.grounded_completion_receipt(0, 1, "done", &[]),
            Err(GroundedCompletionIssue::PendingObligations(_))
        ));

        let mut truncated = AgentTaskContract::default();
        for index in 0..13 {
            truncated.require_tool_success(format!("required-{index:02}"));
        }
        assert_eq!(
            truncated.grounded_completion_receipt(0, 1, "done", &[]),
            Err(GroundedCompletionIssue::TruncatedLedger)
        );

        let mut unrelated = AgentTaskContract::default();
        for index in 0..20 {
            unrelated.record_tool_outcome(
                &format!("read-{index}"),
                "{}",
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
            );
        }
        for index in 0..10 {
            assert!(unrelated.observe_completion_candidate(
                0,
                index,
                "candidate",
                OutcomeClaimDecision::Accepted,
            ));
        }
        assert!(unrelated
            .grounded_completion_receipt(0, 20, "done", &[])
            .is_ok());
    }

    #[test]
    fn bounded_projection_does_not_block_a_long_fully_verified_workspace_task() {
        let mut contract = AgentTaskContract::default();
        contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        for index in 0..10 {
            contract.record_tool_outcome(
                "file.write",
                &format!(r#"{{"path":"src/file-{index}.rs"}}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::WritesWorkspace),
            );
            contract.record_tool_outcome(
                "process.run",
                r#"{"command":"cargo test"}"#,
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ExecutesProcess),
            );
        }
        let ledger = contract.outcome_ledger_shadow(0);
        assert!(ledger.truncation.postconditions > 0);
        assert!(ledger
            .postconditions
            .iter()
            .all(|item| { !item.required || item.status == OutcomePostconditionStatus::Verified }));
        let visible = contract.grounded_completion_required_evidence_sequences(0);
        assert!(contract
            .grounded_completion_receipt(0, 20, "verified", &visible)
            .is_ok());
    }

    #[test]
    fn receipt_keeps_only_required_evidence_visible_to_the_request() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("file.read");
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"src/lib.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        let sequence = contract.evidence()[0].sequence;
        assert_eq!(
            contract.grounded_completion_receipt(0, 1, "grounded", &[]),
            Err(GroundedCompletionIssue::EvidenceNotVisible(vec![sequence]))
        );
        let receipt = contract
            .grounded_completion_receipt(0, 1, "grounded", &[sequence, sequence + 999])
            .expect("visible evidence should cover the obligation");
        assert_eq!(receipt.basis, GroundedCompletionBasis::EvidenceVisible);
        assert_eq!(receipt.visible_evidence_sequences, vec![sequence]);
        assert_eq!(
            contract.grounded_completion_required_evidence_sequences(0),
            vec![sequence]
        );
    }

    #[test]
    fn blocked_completion_requires_visible_denial_and_honest_disclosure() {
        let mut contract = AgentTaskContract::default();
        contract.begin_action_denial_epoch(4);
        contract.require_tool_success("file.write");
        let input_fingerprint =
            crate::tool_input_fingerprint("file.write", r#"{"path":"protected.txt"}"#);
        let denial = contract
            .record_action_denial(
                "file.write",
                &input_fingerprint,
                &crate::AgentActionDenialFeedback::user_permission(),
            )
            .expect("permission denial should be trusted");

        assert_eq!(
            contract.grounded_completion_receipt(4, 2, "done", &[denial.evidence_sequence]),
            Err(GroundedCompletionIssue::MissingConstraintDisclosure(vec![
                "user_permission_denied".to_string()
            ]))
        );
        assert_eq!(
            contract.grounded_completion_receipt(4, 2, "Permission denied.", &[]),
            Err(GroundedCompletionIssue::EvidenceNotVisible(vec![
                denial.evidence_sequence
            ]))
        );

        let receipt = contract
            .grounded_completion_receipt(
                4,
                2,
                "Permission denied, so protected.txt was not changed.",
                &[denial.evidence_sequence],
            )
            .expect("an honest blocked result should be deliverable");
        assert_eq!(receipt.basis, GroundedCompletionBasis::ConstraintObserved);
        assert_eq!(
            receipt.constraint_codes,
            vec!["user_permission_denied".to_string()]
        );
        assert_eq!(
            GroundedCompletionReceipt::from_terminal_metadata(
                &completed_metadata(
                    &contract,
                    &receipt,
                    "Permission denied, so protected.txt was not changed.",
                ),
                "Permission denied, so protected.txt was not changed.",
            ),
            Some(receipt)
        );
    }

    #[test]
    fn verified_postcondition_requires_action_and_observation_visibility() {
        let mut contract = AgentTaskContract::default();
        contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/lib.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        assert!(matches!(
            contract.grounded_completion_receipt(0, 1, "done", &[]),
            Err(GroundedCompletionIssue::PendingPostconditions(_))
        ));
        contract.record_tool_outcome(
            "process.run",
            r#"{"command":"cargo test"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ExecutesProcess),
        );
        let sequences = contract
            .evidence()
            .iter()
            .map(|item| item.sequence)
            .collect::<Vec<_>>();
        assert!(matches!(
            contract.grounded_completion_receipt(0, 2, "done", &sequences[1..]),
            Err(GroundedCompletionIssue::EvidenceNotVisible(_))
        ));
        let receipt = contract
            .grounded_completion_receipt(0, 2, "done", &sequences)
            .expect("verified action and observation should complete");
        assert_eq!(
            receipt.basis,
            GroundedCompletionBasis::PostconditionVerified
        );
        assert_eq!(receipt.covered_obligation_ids.len(), 2);
        assert_eq!(
            receipt.visible_evidence_sequences,
            contract.grounded_completion_required_evidence_sequences(0)
        );
        assert_eq!(receipt.visible_evidence_sequences.len(), 2);
    }

    #[test]
    fn rebind_changes_only_exact_content_and_preserves_the_full_lineage() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("file.read");
        for tool in ["file.read", "file.search"] {
            contract.record_tool_outcome(
                tool,
                r#"{"path":"src"}"#,
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
            );
        }
        let sequences = contract
            .evidence()
            .iter()
            .map(|item| item.sequence)
            .collect::<Vec<_>>();
        let original = contract
            .grounded_completion_receipt(7, 2, "draft", &sequences[..1])
            .expect("draft should bind");
        let rebound = contract
            .rebind_grounded_completion_receipt(&original, 7, 3, "final bytes", &sequences)
            .expect("extra visible evidence should not change the required lineage");
        assert_eq!(rebound.content_sha256, fingerprint("final bytes"));
        assert_eq!(rebound.visible_evidence_sequences, sequences[..1]);
        assert_eq!(
            contract.rebind_grounded_completion_receipt(&original, 7, 3, "final bytes", &[]),
            Err(GroundedCompletionIssue::EvidenceNotVisible(vec![
                sequences[0]
            ]))
        );
    }

    #[test]
    fn metadata_tampering_is_rejected() {
        let receipt = AgentTaskContract::default()
            .grounded_completion_receipt(0, 1, "answer", &[])
            .expect("receipt should build");
        let mut metadata = Metadata::new();
        metadata.insert("steer_epoch".to_string(), "0".to_string());
        assert!(receipt.insert_metadata(&mut metadata));
        metadata
            .get_mut(GROUNDED_COMPLETION_METADATA_KEY)
            .expect("encoded receipt")
            .push(' ');
        assert!(GroundedCompletionReceipt::from_metadata(&metadata).is_none());
    }

    #[test]
    fn strict_terminal_validation_binds_answer_ledger_and_outer_epoch() {
        let contract = AgentTaskContract::default();
        let receipt = contract
            .grounded_completion_receipt(3, 4, "exact answer", &[])
            .expect("receipt should build");
        let metadata = completed_metadata(&contract, &receipt, "exact answer");

        assert_eq!(
            GroundedCompletionReceipt::from_terminal_metadata(&metadata, "exact answer"),
            Some(receipt)
        );
        assert!(
            GroundedCompletionReceipt::from_terminal_metadata(&metadata, "other answer").is_none()
        );
        let mut missing_epoch = metadata.clone();
        missing_epoch.remove("steer_epoch");
        assert!(
            GroundedCompletionReceipt::from_terminal_metadata(&missing_epoch, "exact answer")
                .is_none()
        );
    }

    #[test]
    fn same_source_grounding_requirements_keep_distinct_receipt_sequences() {
        let mut contract = AgentTaskContract::default();
        contract.replace_prompt_evidence_requirements(
            4,
            [
                (
                    "workspace_a".to_string(),
                    BTreeSet::from(["file.read".to_string()]),
                ),
                (
                    "workspace_b".to_string(),
                    BTreeSet::from(["file.read".to_string()]),
                ),
            ]
            .into_iter()
            .collect(),
        );
        assert!(contract.record_prompt_tool_evidence_observation_at(
            4,
            "file.read",
            "file.read",
            r#"{"path":"README.md"}"#,
            "substantive workspace evidence",
        ));
        let sequences = contract
            .outcome_ledger_shadow(4)
            .obligations
            .iter()
            .filter_map(|item| item.evidence_sequence)
            .collect::<BTreeSet<_>>();

        assert_eq!(sequences.len(), 2);
        let receipt = contract
            .grounded_completion_receipt(
                4,
                2,
                "grounded",
                &sequences.iter().copied().collect::<Vec<_>>(),
            )
            .expect("both exact grounding sequences should complete");
        assert_eq!(receipt.visible_evidence_sequences.len(), 2);
    }

    #[test]
    fn steer_evidence_cannot_complete_a_different_epoch() {
        let mut contract = AgentTaskContract::default();
        contract.replace_prompt_evidence_requirement(2, Some("workspace"), ["file.read"]);
        assert!(contract.record_prompt_tool_evidence_observation_at(
            2,
            "file.read",
            "file.read",
            r#"{"path":"README.md"}"#,
            "substantive workspace evidence",
        ));
        let sequences = contract.grounded_completion_required_evidence_sequences(2);

        assert_eq!(
            contract.grounded_completion_receipt(1, 2, "stale", &sequences),
            Err(GroundedCompletionIssue::InvalidLedger)
        );
    }

    #[test]
    fn retained_contract_survives_a_later_noop_control_epoch() {
        let mut contract = AgentTaskContract::default();
        contract.replace_prompt_required_tool_successes(0, ["image.generate"]);
        contract.record_tool_outcome(
            "image.generate",
            r#"{"prompt":"lighthouse"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        let sequences = contract.grounded_completion_required_evidence_sequences(4);
        let receipt = contract
            .grounded_completion_receipt(4, 2, "generated", &sequences)
            .expect("a no-op control epoch must retain the satisfied objective contract");
        assert_eq!(receipt.steer_epoch, 4);
        assert_eq!(receipt.contract_epoch, 0);
        let metadata = completed_metadata(&contract, &receipt, "generated");
        assert_eq!(
            GroundedCompletionReceipt::from_terminal_metadata(&metadata, "generated"),
            Some(receipt)
        );
    }

    #[test]
    fn strict_terminal_validation_rejects_wrong_obligation_evidence_kind() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("file.read");
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        let sequences = contract.grounded_completion_required_evidence_sequences(0);
        let receipt = contract
            .grounded_completion_receipt(0, 2, "grounded", &sequences)
            .expect("receipt should build");
        let mut metadata = completed_metadata(&contract, &receipt, "grounded");
        let mut ledger = contract.completed_outcome_ledger(OutcomeTerminalObservation {
            steer_epoch: 0,
            model_turn: 2,
            answer: "grounded",
            selected_stage: "executor",
            selector_quality: ResultQuality::Grounded,
            selector_marked_verified: false,
            selector_marked_deliverable: true,
            selector_evidence_count: sequences.len(),
            trusted_evidence_sequences: &sequences,
        });
        ledger.evidence[0].kind = ContractEvidenceKind::Read;
        assert!(ledger.insert_metadata(&mut metadata));

        assert!(GroundedCompletionReceipt::from_terminal_metadata(&metadata, "grounded").is_none());
    }

    #[test]
    fn strict_terminal_validation_requires_receipt_evidence_in_delivered_claim() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("file.read");
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        let sequences = contract.grounded_completion_required_evidence_sequences(0);
        let receipt = contract
            .grounded_completion_receipt(0, 2, "grounded", &sequences)
            .expect("receipt should build");
        let mut metadata = completed_metadata(&contract, &receipt, "grounded");
        let mut ledger = contract.completed_outcome_ledger(OutcomeTerminalObservation {
            steer_epoch: 0,
            model_turn: 2,
            answer: "grounded",
            selected_stage: "executor",
            selector_quality: ResultQuality::Grounded,
            selector_marked_verified: false,
            selector_marked_deliverable: true,
            selector_evidence_count: 0,
            trusted_evidence_sequences: &[],
        });
        let delivered = ledger
            .claims
            .iter_mut()
            .find(|claim| claim.kind == OutcomeClaimKind::DeliveredAnswer)
            .expect("delivered claim");
        delivered.evidence_status = OutcomeClaimEvidenceStatus::Unobserved;
        delivered.available_evidence_sequences.clear();
        assert!(ledger.insert_metadata(&mut metadata));

        assert!(GroundedCompletionReceipt::from_terminal_metadata(&metadata, "grounded").is_none());
    }
}
