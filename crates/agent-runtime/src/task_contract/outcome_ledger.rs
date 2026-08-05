use super::{
    denial::stable_denial_code_is_valid, fingerprint, interaction_action, interaction_observation,
    interaction_observation_verifies, AgentActionDenial, AgentActionDenialKind, AgentTaskContract,
    ContractEvidenceKind, WorkspaceVerificationPolicy,
};
use crate::{AgentFailure, AgentFailureClass, InteractionSurface, ResultQuality};
use agent_core::Metadata;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const OUTCOME_LEDGER_SCHEMA: &str = "cindx.agent.outcome-ledger.v1";
pub const OUTCOME_LEDGER_METADATA_KEY: &str = "outcome_ledger_v1";
pub const OUTCOME_LEDGER_DIGEST_METADATA_KEY: &str = "outcome_ledger_digest";
pub const OUTCOME_LEDGER_MAX_METADATA_BYTES: usize = 12 * 1024;

const MAX_OUTCOME_OBLIGATIONS: usize = 12;
const MAX_OUTCOME_EVIDENCE: usize = 16;
const MAX_OUTCOME_CLAIMS: usize = 8;
const MAX_OUTCOME_POSTCONDITIONS: usize = 8;
const MAX_SOURCE_BYTES: usize = 128;
const MAX_STAGE_BYTES: usize = 64;
const MAX_FAILURE_CODE_BYTES: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeLedgerPhase {
    Active,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeObligationKind {
    RequiredTool,
    AnyTool,
    Grounding,
    WorkspaceVerification,
    InteractionObservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeScope {
    Run,
    Steer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeSatisfaction {
    Pending,
    Satisfied,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeBlocker {
    pub kind: AgentActionDenialKind,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeObligation {
    pub id: String,
    pub kind: OutcomeObligationKind,
    pub scope: OutcomeScope,
    pub steer_epoch: Option<u64>,
    pub satisfaction: OutcomeSatisfaction,
    pub evidence_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<OutcomeBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeEvidence {
    pub sequence: u64,
    pub kind: ContractEvidenceKind,
    pub source: String,
    pub input_fingerprint: String,
    pub available_to_terminal_context: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeClaimKind {
    CompletionCandidate,
    DeliveredAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeClaimDecision {
    Accepted,
    RepairRequired,
    ContractFailed,
    Delivered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeClaimEvidenceStatus {
    Unobserved,
    AvailableNotEntailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeClaim {
    pub sequence: u64,
    pub kind: OutcomeClaimKind,
    pub steer_epoch: u64,
    pub model_turn: u64,
    pub content_sha256: String,
    pub content_bytes: u64,
    pub decision: OutcomeClaimDecision,
    pub evidence_status: OutcomeClaimEvidenceStatus,
    pub available_evidence_sequences: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomePostconditionKind {
    WorkspaceMutation,
    BrowserInteraction,
    ComputerInteraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomePostconditionStatus {
    Pending,
    Verified,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomePostcondition {
    pub id: String,
    pub kind: OutcomePostconditionKind,
    pub required: bool,
    pub status: OutcomePostconditionStatus,
    pub action_sequence: u64,
    pub action_source: String,
    pub observation_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeClaimQuality {
    Draft,
    Substantive,
    Grounded,
    Verified,
    Synthesized,
}

impl From<ResultQuality> for OutcomeClaimQuality {
    fn from(value: ResultQuality) -> Self {
        match value {
            ResultQuality::Draft => Self::Draft,
            ResultQuality::Substantive => Self::Substantive,
            ResultQuality::Grounded => Self::Grounded,
            ResultQuality::Verified => Self::Verified,
            ResultQuality::Synthesized => Self::Synthesized,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeTerminal {
    pub selected_stage: String,
    pub selector_quality: OutcomeClaimQuality,
    pub selector_marked_verified: bool,
    pub selector_marked_deliverable: bool,
    pub selector_evidence_count: u64,
    pub delivered_claim_sequence: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct OutcomeTerminalObservation<'a> {
    pub steer_epoch: u64,
    pub model_turn: usize,
    pub answer: &'a str,
    pub selected_stage: &'a str,
    pub selector_quality: ResultQuality,
    pub selector_marked_verified: bool,
    pub selector_marked_deliverable: bool,
    pub selector_evidence_count: usize,
    pub trusted_evidence_sequences: &'a [u64],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeFailureClass {
    Cancelled,
    Budget,
    Contract,
    ModelOutput,
    ProviderTransient,
    ProviderPermanent,
    Tool,
    Internal,
}

impl From<AgentFailureClass> for OutcomeFailureClass {
    fn from(value: AgentFailureClass) -> Self {
        match value {
            AgentFailureClass::Cancelled => Self::Cancelled,
            AgentFailureClass::Budget => Self::Budget,
            AgentFailureClass::Contract => Self::Contract,
            AgentFailureClass::ModelOutput => Self::ModelOutput,
            AgentFailureClass::ProviderTransient => Self::ProviderTransient,
            AgentFailureClass::ProviderPermanent => Self::ProviderPermanent,
            AgentFailureClass::Tool => Self::Tool,
            AgentFailureClass::Internal => Self::Internal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeFailure {
    pub code: String,
    pub class: OutcomeFailureClass,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeTruncation {
    pub obligations: u64,
    pub evidence: u64,
    pub claims: u64,
    pub postconditions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeLedgerShadow {
    pub schema: String,
    pub steer_epoch: u64,
    pub phase: OutcomeLedgerPhase,
    pub obligations: Vec<OutcomeObligation>,
    pub evidence: Vec<OutcomeEvidence>,
    pub claims: Vec<OutcomeClaim>,
    pub postconditions: Vec<OutcomePostcondition>,
    pub terminal: Option<OutcomeTerminal>,
    pub failure: Option<OutcomeFailure>,
    pub truncation: OutcomeTruncation,
}

impl OutcomeLedgerShadow {
    pub fn insert_metadata(&self, metadata: &mut Metadata) -> bool {
        let Some((encoded, digest)) = self.metadata_values() else {
            return false;
        };
        metadata.insert(OUTCOME_LEDGER_METADATA_KEY.to_string(), encoded);
        metadata.insert(OUTCOME_LEDGER_DIGEST_METADATA_KEY.to_string(), digest);
        true
    }

    pub fn from_terminal_metadata(
        metadata: &Metadata,
        expected_phase: OutcomeLedgerPhase,
    ) -> Option<Self> {
        if expected_phase == OutcomeLedgerPhase::Active
            || metadata.get("outcome_ledger_status").map(String::as_str) != Some("recorded")
        {
            return None;
        }
        let outer_steer_epoch = metadata.get("steer_epoch")?.parse::<u64>().ok()?;
        let ledger = Self::from_metadata(metadata)?;
        (ledger.phase == expected_phase && ledger.steer_epoch == outer_steer_epoch)
            .then_some(ledger)
    }

    fn from_metadata(metadata: &Metadata) -> Option<Self> {
        let encoded = metadata.get(OUTCOME_LEDGER_METADATA_KEY)?;
        let expected_digest = metadata.get(OUTCOME_LEDGER_DIGEST_METADATA_KEY)?;
        if encoded.len() > OUTCOME_LEDGER_MAX_METADATA_BYTES
            || !is_sha256_hex(expected_digest)
            || fingerprint(encoded) != *expected_digest
        {
            return None;
        }
        let ledger = serde_json::from_str::<Self>(encoded).ok()?;
        ledger.contract_is_valid().then_some(ledger)
    }

    pub fn contract_is_valid(&self) -> bool {
        self.semantic_contract_is_valid()
            && serde_json::to_vec(self)
                .map(|encoded| encoded.len() <= OUTCOME_LEDGER_MAX_METADATA_BYTES)
                .unwrap_or(false)
    }

    fn metadata_values(&self) -> Option<(String, String)> {
        if !self.semantic_contract_is_valid() {
            return None;
        }
        let encoded = serde_json::to_string(self).ok()?;
        if encoded.len() > OUTCOME_LEDGER_MAX_METADATA_BYTES {
            return None;
        }
        let digest = fingerprint(&encoded);
        Some((encoded, digest))
    }

    fn semantic_contract_is_valid(&self) -> bool {
        if self.schema != OUTCOME_LEDGER_SCHEMA
            || self.obligations.len() > MAX_OUTCOME_OBLIGATIONS
            || self.evidence.len() > MAX_OUTCOME_EVIDENCE
            || self.claims.len() > MAX_OUTCOME_CLAIMS
            || self.postconditions.len() > MAX_OUTCOME_POSTCONDITIONS
            || !strictly_increasing(self.evidence.iter().map(|item| item.sequence))
            || !strictly_increasing(self.claims.iter().map(|item| item.sequence))
        {
            return false;
        }
        let evidence = self
            .evidence
            .iter()
            .map(|item| item.sequence)
            .collect::<BTreeSet<_>>();
        let evidence_kinds = self
            .evidence
            .iter()
            .map(|item| (item.sequence, item.kind))
            .collect::<BTreeMap<_, _>>();
        let available_terminal_evidence = self
            .evidence
            .iter()
            .filter(|item| item.available_to_terminal_context)
            .map(|item| item.sequence)
            .collect::<BTreeSet<_>>();
        if self.obligations.iter().any(|item| {
            !is_sha256_hex(&item.id)
                || item.steer_epoch.is_some() != (item.scope == OutcomeScope::Steer)
                || match item.satisfaction {
                    OutcomeSatisfaction::Pending => {
                        item.evidence_sequence.is_some() || item.blocker.is_some()
                    }
                    OutcomeSatisfaction::Satisfied => item.blocker.is_some(),
                    OutcomeSatisfaction::Blocked => {
                        item.evidence_sequence.is_none()
                            || item.evidence_sequence.is_some_and(|sequence| {
                                evidence_kinds.get(&sequence) != Some(&ContractEvidenceKind::Denial)
                            })
                            || item
                                .blocker
                                .as_ref()
                                .is_none_or(|blocker| !stable_denial_code_is_valid(&blocker.code))
                    }
                }
                || item
                    .evidence_sequence
                    .is_some_and(|sequence| !evidence.contains(&sequence))
        }) || self.evidence.iter().any(|item| {
            item.source.trim().is_empty()
                || item.source.len() > MAX_SOURCE_BYTES
                || !is_sha256_hex(&item.input_fingerprint)
        }) || self.claims.iter().any(|item| {
            item.content_bytes == 0
                || !is_sha256_hex(&item.content_sha256)
                || item.steer_epoch > self.steer_epoch
                || item.available_evidence_sequences.len() > MAX_OUTCOME_EVIDENCE
                || !strictly_increasing(item.available_evidence_sequences.iter().copied())
                || item
                    .available_evidence_sequences
                    .iter()
                    .any(|sequence| !available_terminal_evidence.contains(sequence))
                || (item.available_evidence_sequences.is_empty()
                    != (item.evidence_status == OutcomeClaimEvidenceStatus::Unobserved))
                || match item.kind {
                    OutcomeClaimKind::CompletionCandidate => {
                        !matches!(
                            item.decision,
                            OutcomeClaimDecision::Accepted
                                | OutcomeClaimDecision::RepairRequired
                                | OutcomeClaimDecision::ContractFailed
                        ) || item.evidence_status != OutcomeClaimEvidenceStatus::Unobserved
                            || !item.available_evidence_sequences.is_empty()
                    }
                    OutcomeClaimKind::DeliveredAnswer => {
                        item.decision != OutcomeClaimDecision::Delivered
                            || item.steer_epoch != self.steer_epoch
                    }
                }
        }) || self.postconditions.iter().any(|item| {
            !is_sha256_hex(&item.id)
                || item.action_source.trim().is_empty()
                || item.action_source.len() > MAX_SOURCE_BYTES
                || !evidence.contains(&item.action_sequence)
                || !matches!(
                    (item.kind, evidence_kinds.get(&item.action_sequence)),
                    (
                        OutcomePostconditionKind::WorkspaceMutation,
                        Some(ContractEvidenceKind::Mutation)
                    ) | (
                        OutcomePostconditionKind::BrowserInteraction
                            | OutcomePostconditionKind::ComputerInteraction,
                        Some(ContractEvidenceKind::InteractionAction)
                    )
                )
                || (item.status == OutcomePostconditionStatus::Verified)
                    != item.observation_sequence.is_some()
                || item.observation_sequence.is_some_and(|sequence| {
                    sequence <= item.action_sequence
                        || !evidence.contains(&sequence)
                        || !matches!(
                            (item.kind, evidence_kinds.get(&sequence)),
                            (
                                OutcomePostconditionKind::WorkspaceMutation,
                                Some(ContractEvidenceKind::Verification)
                            ) | (
                                OutcomePostconditionKind::BrowserInteraction
                                    | OutcomePostconditionKind::ComputerInteraction,
                                Some(ContractEvidenceKind::InteractionObservation)
                            )
                        )
                })
        }) {
            return false;
        }

        match (&self.phase, &self.terminal, &self.failure) {
            (OutcomeLedgerPhase::Active, None, None) => {
                self.claims
                    .iter()
                    .all(|claim| claim.kind == OutcomeClaimKind::CompletionCandidate)
                    && available_terminal_evidence.is_empty()
            }
            (OutcomeLedgerPhase::Completed, Some(terminal), None) => {
                !terminal.selected_stage.trim().is_empty()
                    && terminal.selected_stage.len() <= MAX_STAGE_BYTES
                    && terminal.selector_marked_deliverable
                    && self
                        .claims
                        .iter()
                        .filter(|claim| claim.kind == OutcomeClaimKind::DeliveredAnswer)
                        .count()
                        == 1
                    && self.claims.iter().any(|claim| {
                        claim.sequence == terminal.delivered_claim_sequence
                            && claim.kind == OutcomeClaimKind::DeliveredAnswer
                            && (!terminal.selector_marked_verified
                                || claim.available_evidence_sequences.iter().any(|sequence| {
                                    matches!(
                                        evidence_kinds.get(sequence),
                                        Some(
                                            ContractEvidenceKind::Verification
                                                | ContractEvidenceKind::InteractionObservation
                                        )
                                    )
                                }))
                    })
            }
            (OutcomeLedgerPhase::Failed, None, Some(failure)) => {
                stable_failure_code_is_valid(&failure.code)
                    && self
                        .claims
                        .iter()
                        .all(|claim| claim.kind == OutcomeClaimKind::CompletionCandidate)
                    && available_terminal_evidence.is_empty()
            }
            _ => false,
        }
    }
}

impl AgentTaskContract {
    pub(crate) fn validate_persisted_outcome_state(&self) -> Result<(), &'static str> {
        self.validate_action_denials()?;
        if self.outcome_claims.len() > MAX_OUTCOME_CLAIMS {
            return Err("too many outcome claims");
        }
        if !strictly_increasing(self.outcome_claims.iter().map(|claim| claim.sequence)) {
            return Err("outcome claim sequences are not strictly increasing");
        }
        if self
            .outcome_dropped_claims
            .checked_add(self.outcome_claims.len() as u64)
            != Some(self.next_outcome_claim_sequence)
            || self
                .outcome_claims
                .iter()
                .enumerate()
                .any(|(index, claim)| {
                    claim.sequence
                        != self
                            .outcome_dropped_claims
                            .saturating_add(index as u64)
                            .saturating_add(1)
                })
        {
            return Err("outcome claim cursor is inconsistent with persisted state");
        }
        if self.outcome_claims.iter().any(|claim| {
            claim.kind != OutcomeClaimKind::CompletionCandidate
                || !matches!(
                    claim.decision,
                    OutcomeClaimDecision::Accepted
                        | OutcomeClaimDecision::RepairRequired
                        | OutcomeClaimDecision::ContractFailed
                )
                || claim.content_bytes == 0
                || !is_sha256_hex(&claim.content_sha256)
                || claim.evidence_status != OutcomeClaimEvidenceStatus::Unobserved
                || !claim.available_evidence_sequences.is_empty()
        }) {
            return Err("outcome claim state is invalid");
        }
        Ok(())
    }

    pub fn observe_completion_candidate(
        &mut self,
        steer_epoch: u64,
        model_turn: usize,
        content: &str,
        decision: OutcomeClaimDecision,
    ) -> bool {
        if content.is_empty()
            || !matches!(
                decision,
                OutcomeClaimDecision::Accepted
                    | OutcomeClaimDecision::RepairRequired
                    | OutcomeClaimDecision::ContractFailed
            )
        {
            return false;
        }
        self.next_outcome_claim_sequence = self.next_outcome_claim_sequence.saturating_add(1);
        self.outcome_claims.push(OutcomeClaim {
            sequence: self.next_outcome_claim_sequence,
            kind: OutcomeClaimKind::CompletionCandidate,
            steer_epoch,
            model_turn: model_turn as u64,
            content_sha256: fingerprint(content),
            content_bytes: content.len() as u64,
            decision,
            evidence_status: OutcomeClaimEvidenceStatus::Unobserved,
            available_evidence_sequences: Vec::new(),
        });
        if self.outcome_claims.len() > MAX_OUTCOME_CLAIMS {
            let excess = self.outcome_claims.len() - MAX_OUTCOME_CLAIMS;
            self.outcome_claims.drain(..excess);
            self.outcome_dropped_claims = self.outcome_dropped_claims.saturating_add(excess as u64);
        }
        true
    }

    pub fn outcome_ledger_shadow(&self, steer_epoch: u64) -> OutcomeLedgerShadow {
        self.build_outcome_ledger(steer_epoch, None, None)
    }

    pub fn completed_outcome_ledger(
        &self,
        terminal: OutcomeTerminalObservation<'_>,
    ) -> OutcomeLedgerShadow {
        self.build_outcome_ledger(terminal.steer_epoch, Some(terminal), None)
    }

    pub fn failed_outcome_ledger(
        &self,
        steer_epoch: u64,
        failure: &AgentFailure,
    ) -> OutcomeLedgerShadow {
        self.build_outcome_ledger(steer_epoch, None, Some(failure))
    }

    fn build_outcome_ledger(
        &self,
        steer_epoch: u64,
        terminal: Option<OutcomeTerminalObservation<'_>>,
        failure: Option<&AgentFailure>,
    ) -> OutcomeLedgerShadow {
        let trusted_sequences = terminal
            .as_ref()
            .map(|terminal| {
                terminal
                    .trusted_evidence_sequences
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let (postconditions, dropped_postconditions) = self.project_postconditions();
        let postcondition_evidence_sequences = postconditions
            .iter()
            .flat_map(|item| std::iter::once(item.action_sequence).chain(item.observation_sequence))
            .collect::<BTreeSet<_>>();
        let (evidence, dropped_evidence) =
            self.project_evidence(&trusted_sequences, &postcondition_evidence_sequences);
        let visible_evidence = evidence
            .iter()
            .map(|item| item.sequence)
            .collect::<BTreeSet<_>>();
        let (mut obligations, dropped_obligations) = self.project_obligations(&postconditions);
        for obligation in &mut obligations {
            if obligation
                .evidence_sequence
                .is_some_and(|sequence| !visible_evidence.contains(&sequence))
            {
                obligation.evidence_sequence = None;
            }
        }
        let mut claims = self.outcome_claims.clone();
        let mut dropped_claims = self.outcome_dropped_claims;
        let terminal_record = terminal.map(|terminal| {
            let available_evidence_sequences = terminal
                .trusted_evidence_sequences
                .iter()
                .copied()
                .filter(|sequence| visible_evidence.contains(sequence))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let delivered_claim_sequence = self.next_outcome_claim_sequence.saturating_add(1);
            claims.push(OutcomeClaim {
                sequence: delivered_claim_sequence,
                kind: OutcomeClaimKind::DeliveredAnswer,
                steer_epoch: terminal.steer_epoch,
                model_turn: terminal.model_turn as u64,
                content_sha256: fingerprint(terminal.answer),
                content_bytes: terminal.answer.len() as u64,
                decision: OutcomeClaimDecision::Delivered,
                evidence_status: if available_evidence_sequences.is_empty() {
                    OutcomeClaimEvidenceStatus::Unobserved
                } else {
                    OutcomeClaimEvidenceStatus::AvailableNotEntailed
                },
                available_evidence_sequences,
            });
            if claims.len() > MAX_OUTCOME_CLAIMS {
                let excess = claims.len() - MAX_OUTCOME_CLAIMS;
                claims.drain(..excess);
                dropped_claims = dropped_claims.saturating_add(excess as u64);
            }
            OutcomeTerminal {
                selected_stage: bounded_stage(terminal.selected_stage),
                selector_quality: terminal.selector_quality.into(),
                selector_marked_verified: terminal.selector_marked_verified,
                selector_marked_deliverable: terminal.selector_marked_deliverable,
                selector_evidence_count: terminal.selector_evidence_count as u64,
                delivered_claim_sequence,
            }
        });

        OutcomeLedgerShadow {
            schema: OUTCOME_LEDGER_SCHEMA.to_string(),
            steer_epoch,
            phase: match (terminal_record.is_some(), failure.is_some()) {
                (true, false) => OutcomeLedgerPhase::Completed,
                (false, true) => OutcomeLedgerPhase::Failed,
                _ => OutcomeLedgerPhase::Active,
            },
            obligations,
            evidence,
            claims,
            postconditions,
            terminal: terminal_record,
            failure: failure.map(|failure| OutcomeFailure {
                code: bounded_failure_code(&failure.code),
                class: failure.class.into(),
            }),
            truncation: OutcomeTruncation {
                obligations: dropped_obligations,
                evidence: dropped_evidence,
                claims: dropped_claims,
                postconditions: dropped_postconditions,
            },
        }
    }

    fn project_evidence(
        &self,
        available_terminal_sequences: &BTreeSet<u64>,
        required_sequences: &BTreeSet<u64>,
    ) -> (Vec<OutcomeEvidence>, u64) {
        let mut selected_sequences = required_sequences
            .iter()
            .copied()
            .take(MAX_OUTCOME_EVIDENCE)
            .collect::<BTreeSet<_>>();
        for item in self.evidence.iter().rev() {
            if selected_sequences.len() >= MAX_OUTCOME_EVIDENCE {
                break;
            }
            selected_sequences.insert(item.sequence);
        }
        let evidence = self
            .evidence
            .iter()
            .filter(|item| selected_sequences.contains(&item.sequence))
            .map(|item| OutcomeEvidence {
                sequence: item.sequence,
                kind: item.kind,
                source: bounded_source(&item.source),
                input_fingerprint: item.input_fingerprint.clone(),
                available_to_terminal_context: available_terminal_sequences
                    .contains(&item.sequence),
            })
            .collect::<Vec<_>>();
        let dropped = self.evidence.len().saturating_sub(evidence.len());
        (evidence, dropped as u64)
    }

    fn project_obligations(
        &self,
        postconditions: &[OutcomePostcondition],
    ) -> (Vec<OutcomeObligation>, u64) {
        let mut obligations = Vec::new();
        for tool in &self.required_tool_successes {
            obligations.push(self.tool_obligation(tool, OutcomeScope::Run, None, false));
        }
        for tool in &self.prompt_required_tool_successes {
            obligations.push(self.tool_obligation(
                tool,
                OutcomeScope::Steer,
                Some(self.prompt_requirement_epoch),
                true,
            ));
        }
        for (requirement, alternatives) in &self.required_any_tool_successes {
            obligations.push(self.any_tool_obligation(
                requirement,
                alternatives,
                OutcomeScope::Run,
                None,
                false,
            ));
        }
        for (requirement, alternatives) in &self.prompt_required_any_tool_successes {
            obligations.push(self.any_tool_obligation(
                requirement,
                alternatives,
                OutcomeScope::Steer,
                Some(self.prompt_requirement_epoch),
                true,
            ));
        }
        for (requirement_id, requirement) in &self.prompt_evidence_requirements {
            let evidence_sequence = requirement.receipt.as_ref().and_then(|receipt| {
                self.evidence
                    .iter()
                    .find(|evidence| {
                        evidence.sequence == receipt.evidence_sequence
                            && evidence.kind == ContractEvidenceKind::Grounding
                            && evidence.source == receipt.source
                    })
                    .map(|evidence| evidence.sequence)
            });
            let denial = evidence_sequence
                .is_none()
                .then(|| self.action_denial_for_tools(requirement.tools.iter().map(String::as_str)))
                .flatten();
            obligations.push(OutcomeObligation {
                id: outcome_id(&format!(
                    "grounding|steer|{}|{requirement_id}",
                    self.prompt_evidence_epoch
                )),
                kind: OutcomeObligationKind::Grounding,
                scope: OutcomeScope::Steer,
                steer_epoch: Some(self.prompt_evidence_epoch),
                satisfaction: obligation_satisfaction(evidence_sequence.is_some(), denial),
                evidence_sequence: evidence_sequence
                    .or_else(|| denial.map(|denial| denial.evidence_sequence)),
                blocker: denial.map(outcome_blocker),
            });
        }
        if self.workspace_verification_policy == WorkspaceVerificationPolicy::RequiredAfterMutation
            && self.mutation_epoch > 0
        {
            obligations.push(OutcomeObligation {
                id: outcome_id(&format!("workspace_verification|{}", self.mutation_epoch)),
                kind: OutcomeObligationKind::WorkspaceVerification,
                scope: OutcomeScope::Run,
                steer_epoch: None,
                satisfaction: if self.latest_mutation_verified() {
                    OutcomeSatisfaction::Satisfied
                } else {
                    OutcomeSatisfaction::Pending
                },
                evidence_sequence: self
                    .latest_mutation_verified()
                    .then(|| {
                        self.latest_evidence_sequence(|evidence| {
                            evidence.kind == ContractEvidenceKind::Verification
                        })
                    })
                    .flatten(),
                blocker: None,
            });
        }
        for postcondition in postconditions
            .iter()
            .filter(|item| item.kind != OutcomePostconditionKind::WorkspaceMutation)
            .filter(|item| item.status != OutcomePostconditionStatus::Superseded)
        {
            obligations.push(OutcomeObligation {
                id: outcome_id(&format!("interaction_observation|{}", postcondition.id)),
                kind: OutcomeObligationKind::InteractionObservation,
                scope: OutcomeScope::Run,
                steer_epoch: None,
                satisfaction: if postcondition.status == OutcomePostconditionStatus::Verified {
                    OutcomeSatisfaction::Satisfied
                } else {
                    OutcomeSatisfaction::Pending
                },
                evidence_sequence: postcondition.observation_sequence,
                blocker: None,
            });
        }
        obligations.sort_by(|left, right| {
            (
                left.satisfaction == OutcomeSatisfaction::Satisfied,
                left.kind,
                &left.id,
            )
                .cmp(&(
                    right.satisfaction == OutcomeSatisfaction::Satisfied,
                    right.kind,
                    &right.id,
                ))
        });
        let dropped = obligations.len().saturating_sub(MAX_OUTCOME_OBLIGATIONS);
        obligations.truncate(MAX_OUTCOME_OBLIGATIONS);
        (obligations, dropped as u64)
    }

    fn tool_obligation(
        &self,
        tool: &str,
        scope: OutcomeScope,
        steer_epoch: Option<u64>,
        prompt_scoped: bool,
    ) -> OutcomeObligation {
        let satisfied = if prompt_scoped {
            self.prompt_successful_tools.contains(tool)
        } else {
            self.successful_tools.contains(tool)
        };
        let denial = (!satisfied)
            .then(|| self.action_denial_for_tools([tool]))
            .flatten();
        OutcomeObligation {
            id: outcome_id(&format!("required_tool|{scope:?}|{steer_epoch:?}|{tool}")),
            kind: OutcomeObligationKind::RequiredTool,
            scope,
            steer_epoch,
            satisfaction: obligation_satisfaction(satisfied, denial),
            evidence_sequence: if satisfied {
                self.latest_evidence_sequence(|evidence| {
                    evidence.kind == ContractEvidenceKind::RequiredTool && evidence.source == tool
                })
            } else {
                denial.map(|denial| denial.evidence_sequence)
            },
            blocker: denial.map(outcome_blocker),
        }
    }

    fn any_tool_obligation(
        &self,
        requirement: &str,
        alternatives: &BTreeSet<String>,
        scope: OutcomeScope,
        steer_epoch: Option<u64>,
        prompt_scoped: bool,
    ) -> OutcomeObligation {
        let successful_tools = if prompt_scoped {
            &self.prompt_successful_tools
        } else {
            &self.successful_tools
        };
        let satisfied = !alternatives.is_disjoint(successful_tools);
        let denial = (!satisfied)
            .then(|| self.action_denial_for_tools(alternatives.iter().map(String::as_str)))
            .flatten();
        OutcomeObligation {
            id: outcome_id(&format!("any_tool|{scope:?}|{steer_epoch:?}|{requirement}")),
            kind: OutcomeObligationKind::AnyTool,
            scope,
            steer_epoch,
            satisfaction: obligation_satisfaction(satisfied, denial),
            evidence_sequence: if satisfied {
                self.latest_evidence_sequence(|evidence| alternatives.contains(&evidence.source))
            } else {
                denial.map(|denial| denial.evidence_sequence)
            },
            blocker: denial.map(outcome_blocker),
        }
    }

    fn latest_evidence_sequence(
        &self,
        predicate: impl Fn(&super::ContractEvidence) -> bool,
    ) -> Option<u64> {
        self.evidence
            .iter()
            .rev()
            .find(|evidence| predicate(evidence))
            .map(|evidence| evidence.sequence)
    }

    fn project_postconditions(&self) -> (Vec<OutcomePostcondition>, u64) {
        let mut postconditions = Vec::<OutcomePostcondition>::new();
        let mut pending_workspace = None;
        let mut pending_interactions = BTreeMap::<InteractionSurface, usize>::new();
        for evidence in &self.evidence {
            match evidence.kind {
                ContractEvidenceKind::Mutation => {
                    if let Some(index) = pending_workspace.replace(postconditions.len()) {
                        postconditions[index].status = OutcomePostconditionStatus::Superseded;
                    }
                    postconditions.push(OutcomePostcondition {
                        id: outcome_id(&format!(
                            "workspace_mutation|{}|{}",
                            evidence.sequence, evidence.source
                        )),
                        kind: OutcomePostconditionKind::WorkspaceMutation,
                        required: self.workspace_verification_policy.is_required(),
                        status: OutcomePostconditionStatus::Pending,
                        action_sequence: evidence.sequence,
                        action_source: evidence.source.clone(),
                        observation_sequence: None,
                    });
                }
                ContractEvidenceKind::Verification => {
                    if let Some(index) = pending_workspace.take() {
                        postconditions[index].status = OutcomePostconditionStatus::Verified;
                        postconditions[index].observation_sequence = Some(evidence.sequence);
                    }
                }
                ContractEvidenceKind::InteractionAction => {
                    let Some((surface, _)) = interaction_action(&evidence.source) else {
                        continue;
                    };
                    if let Some(index) = pending_interactions.insert(surface, postconditions.len())
                    {
                        postconditions[index].status = OutcomePostconditionStatus::Superseded;
                    }
                    postconditions.push(OutcomePostcondition {
                        id: outcome_id(&format!(
                            "interaction|{}|{}",
                            evidence.sequence, evidence.source
                        )),
                        kind: match surface {
                            InteractionSurface::Browser => {
                                OutcomePostconditionKind::BrowserInteraction
                            }
                            InteractionSurface::Computer => {
                                OutcomePostconditionKind::ComputerInteraction
                            }
                        },
                        required: true,
                        status: OutcomePostconditionStatus::Pending,
                        action_sequence: evidence.sequence,
                        action_source: evidence.source.clone(),
                        observation_sequence: None,
                    });
                }
                ContractEvidenceKind::InteractionObservation => {
                    let Some(surface) = interaction_observation(&evidence.source) else {
                        continue;
                    };
                    let Some(index) = pending_interactions.get(&surface).copied() else {
                        continue;
                    };
                    if interaction_observation_verifies(
                        &postconditions[index].action_source,
                        &evidence.source,
                    ) {
                        postconditions[index].status = OutcomePostconditionStatus::Verified;
                        postconditions[index].observation_sequence = Some(evidence.sequence);
                        pending_interactions.remove(&surface);
                    }
                }
                _ => {}
            }
        }
        postconditions.sort_by_key(|item| item.action_sequence);
        let dropped = postconditions
            .len()
            .saturating_sub(MAX_OUTCOME_POSTCONDITIONS);
        if dropped > 0 {
            postconditions.drain(..dropped);
        }
        for postcondition in &mut postconditions {
            postcondition.action_source = bounded_source(&postcondition.action_source);
        }
        (postconditions, dropped as u64)
    }
}

fn outcome_id(value: &str) -> String {
    fingerprint(value)
}

fn obligation_satisfaction(
    satisfied: bool,
    denial: Option<&AgentActionDenial>,
) -> OutcomeSatisfaction {
    if satisfied {
        OutcomeSatisfaction::Satisfied
    } else if denial.is_some() {
        OutcomeSatisfaction::Blocked
    } else {
        OutcomeSatisfaction::Pending
    }
}

fn outcome_blocker(denial: &AgentActionDenial) -> OutcomeBlocker {
    OutcomeBlocker {
        kind: denial.kind,
        code: denial.code.clone(),
    }
}

fn bounded_stage(value: &str) -> String {
    value
        .trim()
        .chars()
        .scan(0usize, |bytes, character| {
            let next = bytes.saturating_add(character.len_utf8());
            (next <= MAX_STAGE_BYTES).then(|| {
                *bytes = next;
                character
            })
        })
        .collect()
}

fn bounded_source(value: &str) -> String {
    let value = value.trim();
    if !value.is_empty() && value.len() <= MAX_SOURCE_BYTES {
        value.to_string()
    } else {
        format!("sha256:{}", fingerprint(value))
    }
}

fn bounded_failure_code(value: &str) -> String {
    let value = value.trim();
    if stable_failure_code_is_valid(value) {
        value.to_string()
    } else {
        format!("sha256:{}", fingerprint(value))
    }
}

fn stable_failure_code_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FAILURE_CODE_BYTES
        && (value.starts_with("sha256:") && is_sha256_hex(&value["sha256:".len()..])
            || value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'-' | b'.')
            }))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn strictly_increasing(values: impl IntoIterator<Item = u64>) -> bool {
    let mut previous = None;
    for value in values {
        if previous.is_some_and(|previous| previous >= value) {
            return false;
        }
        previous = Some(value);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{ToolOutcomeStatus, ToolRisk, ToolSpec};

    fn tool(name: &str, risk: ToolRisk) -> ToolSpec {
        ToolSpec::builtin(name, "test", "test", risk, r#"{"type":"object"}"#)
    }

    #[test]
    fn model_text_is_a_claim_and_never_evidence() {
        let sentinel = "SENTINEL: I wrote the file and all tests passed";
        let mut contract = AgentTaskContract::default();
        let mut control = contract.clone();
        let gate_without_shadow = control.completion_instruction_for_task(&[]);
        assert!(contract.observe_completion_candidate(
            0,
            1,
            sentinel,
            OutcomeClaimDecision::Accepted,
        ));
        assert_eq!(
            contract.completion_instruction_for_task(&[]),
            gate_without_shadow,
            "shadow observation must not change the completion gate"
        );
        let ledger = contract.outcome_ledger_shadow(0);

        assert!(ledger.evidence.is_empty());
        assert!(ledger.postconditions.is_empty());
        assert_eq!(ledger.claims.len(), 1);
        assert_eq!(ledger.claims[0].content_sha256, fingerprint(sentinel));
        let mut metadata = Metadata::new();
        assert!(ledger.insert_metadata(&mut metadata));
        assert!(!metadata[OUTCOME_LEDGER_METADATA_KEY].contains(sentinel));
        assert_eq!(OutcomeLedgerShadow::from_metadata(&metadata), Some(ledger));
    }

    #[test]
    fn mutation_and_verification_mirror_existing_postcondition_gate() {
        let tools = [
            tool("file.write", ToolRisk::WritesWorkspace),
            tool("process.run", ToolRisk::ExecutesProcess),
        ];
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
        assert!(contract
            .completion_instruction_for_task(&tools)
            .expect("gate evaluates")
            .is_some());
        let pending = contract.outcome_ledger_shadow(0);
        assert_eq!(pending.postconditions.len(), 1);
        assert_eq!(
            pending.postconditions[0].status,
            OutcomePostconditionStatus::Pending
        );

        contract.record_tool_outcome(
            "process.run",
            r#"{"command":"cargo test"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ExecutesProcess),
        );
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
        let verified = contract.outcome_ledger_shadow(0);
        assert_eq!(
            verified.postconditions[0].status,
            OutcomePostconditionStatus::Verified
        );
        assert!(verified.postconditions[0].observation_sequence.is_some());
        assert!(verified.obligations.iter().any(|obligation| {
            obligation.kind == OutcomeObligationKind::WorkspaceVerification
                && obligation.satisfaction == OutcomeSatisfaction::Satisfied
        }));
        let verified_sequences = contract
            .evidence()
            .iter()
            .map(|evidence| evidence.sequence)
            .collect::<Vec<_>>();
        let verified_terminal = contract.completed_outcome_ledger(OutcomeTerminalObservation {
            steer_epoch: 0,
            model_turn: 1,
            answer: "verified",
            selected_stage: "verified_executor",
            selector_quality: ResultQuality::Verified,
            selector_marked_verified: true,
            selector_marked_deliverable: true,
            selector_evidence_count: verified_sequences.len(),
            trusted_evidence_sequences: &verified_sequences,
        });
        assert!(verified_terminal.contract_is_valid());

        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/new.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        let pending_again = contract.outcome_ledger_shadow(0);
        let workspace_obligation = pending_again
            .obligations
            .iter()
            .find(|obligation| obligation.kind == OutcomeObligationKind::WorkspaceVerification)
            .expect("workspace obligation remains projected");
        assert_eq!(
            workspace_obligation.satisfaction,
            OutcomeSatisfaction::Pending
        );
        assert_eq!(
            workspace_obligation.evidence_sequence, None,
            "an earlier verification must not support a later mutation"
        );
    }

    #[test]
    fn failed_or_denied_tools_do_not_fabricate_evidence() {
        let tools = [tool("shell.run", ToolRisk::ExecutesProcess)];
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("shell.run");
        for status in [ToolOutcomeStatus::Failed, ToolOutcomeStatus::Denied] {
            contract.record_tool_outcome(
                "shell.run",
                r#"{"command":"false"}"#,
                &status,
                Some(&ToolRisk::ExecutesProcess),
            );
        }
        contract.observe_completion_candidate(
            0,
            1,
            "Everything succeeded.",
            OutcomeClaimDecision::ContractFailed,
        );
        let ledger = contract.outcome_ledger_shadow(0);

        assert!(ledger.evidence.is_empty());
        assert_eq!(
            ledger.obligations[0].satisfaction,
            OutcomeSatisfaction::Pending
        );
        assert_eq!(
            ledger.claims[0].decision,
            OutcomeClaimDecision::ContractFailed
        );
        assert!(contract
            .completion_instruction_for_task(&tools)
            .expect("first repair remains available")
            .is_some());
    }

    #[test]
    fn failed_ledger_records_typed_reason_without_error_text() {
        let sentinel = "SENSITIVE_PROVIDER_ERROR_SENTINEL";
        let mut contract = AgentTaskContract::default();
        contract.observe_completion_candidate(
            4,
            2,
            "candidate",
            OutcomeClaimDecision::ContractFailed,
        );
        let failure = AgentFailure::contract("required_evidence_unavailable", sentinel);
        let ledger = contract.failed_outcome_ledger(4, &failure);

        assert_eq!(ledger.phase, OutcomeLedgerPhase::Failed);
        assert!(ledger.terminal.is_none());
        assert_eq!(
            ledger.failure,
            Some(OutcomeFailure {
                code: "required_evidence_unavailable".to_string(),
                class: OutcomeFailureClass::Contract,
            })
        );
        let mut metadata = Metadata::new();
        assert!(ledger.insert_metadata(&mut metadata));
        metadata.insert("outcome_ledger_status".to_string(), "recorded".to_string());
        metadata.insert("steer_epoch".to_string(), "4".to_string());
        assert!(!metadata[OUTCOME_LEDGER_METADATA_KEY].contains(sentinel));
        assert_eq!(
            OutcomeLedgerShadow::from_terminal_metadata(&metadata, OutcomeLedgerPhase::Failed),
            Some(ledger.clone())
        );

        let mut invalid = ledger;
        invalid.phase = OutcomeLedgerPhase::Completed;
        assert!(!invalid.contract_is_valid());
    }

    #[test]
    fn steer_scoped_obligations_and_claims_remain_epoch_isolated() {
        let mut contract = AgentTaskContract::default();
        contract.replace_prompt_required_tool_successes(1, ["file.read"]);
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"old"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        contract.observe_completion_candidate(
            1,
            1,
            "old objective",
            OutcomeClaimDecision::Accepted,
        );
        contract.replace_prompt_required_tool_successes(2, ["file.read"]);
        contract.observe_completion_candidate(
            2,
            2,
            "new objective before evidence",
            OutcomeClaimDecision::RepairRequired,
        );

        let ledger = contract.outcome_ledger_shadow(2);
        assert_eq!(ledger.claims[0].steer_epoch, 1);
        assert_eq!(ledger.claims[1].steer_epoch, 2);
        assert!(ledger.obligations.iter().any(|obligation| {
            obligation.scope == OutcomeScope::Steer
                && obligation.steer_epoch == Some(2)
                && obligation.satisfaction == OutcomeSatisfaction::Pending
        }));
    }

    #[test]
    fn completed_ledger_links_only_explicitly_trusted_current_evidence() {
        let mut contract = AgentTaskContract::default();
        for (name, input) in [("file.read", "old"), ("web.search", "current")] {
            contract.record_tool_outcome(
                name,
                input,
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
            );
        }
        contract.observe_completion_candidate(3, 2, "candidate", OutcomeClaimDecision::Accepted);
        let ledger = contract.completed_outcome_ledger(OutcomeTerminalObservation {
            steer_epoch: 3,
            model_turn: 2,
            answer: "delivered",
            selected_stage: "executor",
            selector_quality: ResultQuality::Grounded,
            selector_marked_verified: false,
            selector_marked_deliverable: true,
            selector_evidence_count: 1,
            trusted_evidence_sequences: &[2],
        });
        let delivered = ledger
            .claims
            .iter()
            .find(|claim| claim.kind == OutcomeClaimKind::DeliveredAnswer)
            .expect("delivered claim exists");

        assert_eq!(delivered.available_evidence_sequences, vec![2]);
        assert_eq!(
            delivered.evidence_status,
            OutcomeClaimEvidenceStatus::AvailableNotEntailed
        );
        assert!(!ledger.evidence[0].available_to_terminal_context);
        assert!(ledger.evidence[1].available_to_terminal_context);
        assert!(ledger.contract_is_valid());
        let mut metadata = Metadata::new();
        assert!(ledger.insert_metadata(&mut metadata));
        metadata.insert("outcome_ledger_status".to_string(), "recorded".to_string());
        metadata.insert("steer_epoch".to_string(), "3".to_string());
        assert_eq!(
            OutcomeLedgerShadow::from_terminal_metadata(&metadata, OutcomeLedgerPhase::Completed),
            Some(ledger.clone())
        );
        metadata.insert("steer_epoch".to_string(), "4".to_string());
        assert!(OutcomeLedgerShadow::from_terminal_metadata(
            &metadata,
            OutcomeLedgerPhase::Completed
        )
        .is_none());
    }

    #[test]
    fn claims_and_projections_are_bounded_and_tamper_evident() {
        let mut contract = AgentTaskContract::default();
        for index in 0..(MAX_OUTCOME_OBLIGATIONS + 2) {
            contract.require_tool_success(format!("required-{index}"));
        }
        for index in 0..(MAX_OUTCOME_CLAIMS + 2) {
            contract.observe_completion_candidate(
                0,
                index,
                &format!("candidate-{index}"),
                OutcomeClaimDecision::RepairRequired,
            );
        }
        for index in 0..(MAX_OUTCOME_EVIDENCE + 2) {
            contract.record_tool_outcome(
                "file.read",
                &format!(r#"{{"path":"{index}"}}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
            );
        }
        for index in 0..(MAX_OUTCOME_POSTCONDITIONS + 2) {
            contract.record_tool_outcome(
                "browser.click",
                &format!(r#"{{"index":{index}}}"#),
                &ToolOutcomeStatus::Succeeded,
                None,
            );
            contract.record_tool_outcome(
                "browser.capture",
                "{}",
                &ToolOutcomeStatus::Succeeded,
                None,
            );
        }
        let available_sequences = contract
            .evidence()
            .iter()
            .map(|evidence| evidence.sequence)
            .collect::<Vec<_>>();
        let ledger = contract.completed_outcome_ledger(OutcomeTerminalObservation {
            steer_epoch: 0,
            model_turn: MAX_OUTCOME_CLAIMS,
            answer: "bounded terminal answer",
            selected_stage: "executor",
            selector_quality: ResultQuality::Grounded,
            selector_marked_verified: false,
            selector_marked_deliverable: true,
            selector_evidence_count: available_sequences.len(),
            trusted_evidence_sequences: &available_sequences,
        });
        assert_eq!(ledger.obligations.len(), MAX_OUTCOME_OBLIGATIONS);
        assert_eq!(
            ledger.truncation.obligations,
            2 + MAX_OUTCOME_POSTCONDITIONS as u64
        );
        assert_eq!(ledger.claims.len(), MAX_OUTCOME_CLAIMS);
        assert_eq!(ledger.truncation.claims, 3);
        assert_eq!(ledger.evidence.len(), MAX_OUTCOME_EVIDENCE);
        assert!(ledger.truncation.evidence >= 2);
        assert_eq!(ledger.postconditions.len(), MAX_OUTCOME_POSTCONDITIONS);
        assert_eq!(ledger.truncation.postconditions, 2);
        assert!(ledger.contract_is_valid());
        let mut metadata = Metadata::new();
        assert!(ledger.insert_metadata(&mut metadata));
        assert!(metadata[OUTCOME_LEDGER_METADATA_KEY].len() <= OUTCOME_LEDGER_MAX_METADATA_BYTES);

        metadata.insert(
            OUTCOME_LEDGER_DIGEST_METADATA_KEY.to_string(),
            "0".repeat(64),
        );
        assert!(OutcomeLedgerShadow::from_metadata(&metadata).is_none());

        let mut semantic_tamper = ledger.clone();
        semantic_tamper.claims[0].content_bytes = 0;
        let encoded = serde_json::to_string(&semantic_tamper).expect("tamper encodes");
        metadata.insert(OUTCOME_LEDGER_METADATA_KEY.to_string(), encoded.clone());
        metadata.insert(
            OUTCOME_LEDGER_DIGEST_METADATA_KEY.to_string(),
            fingerprint(&encoded),
        );
        assert!(OutcomeLedgerShadow::from_metadata(&metadata).is_none());
    }

    #[test]
    fn long_tool_sources_are_bounded_without_losing_identity() {
        let long_source = format!("mcp.{}", "x".repeat(512));
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            &long_source,
            "{}",
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        let ledger = contract.completed_outcome_ledger(OutcomeTerminalObservation {
            steer_epoch: 0,
            model_turn: 1,
            answer: "done",
            selected_stage: "executor",
            selector_quality: ResultQuality::Grounded,
            selector_marked_verified: false,
            selector_marked_deliverable: true,
            selector_evidence_count: 1,
            trusted_evidence_sequences: &[1],
        });

        assert!(ledger.evidence[0].source.starts_with("sha256:"));
        assert!(!ledger.evidence[0].source.contains(&long_source));
        let mut metadata = Metadata::new();
        assert!(ledger.insert_metadata(&mut metadata));
        assert!(metadata[OUTCOME_LEDGER_METADATA_KEY].len() <= OUTCOME_LEDGER_MAX_METADATA_BYTES);
    }
}
