use crate::claim_evidence::{CitationStatus, ClaimEvidenceReceipt};
use crate::{GroundedCompletionBasis, GroundedCompletionReceipt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

pub const DELIVERY_VERIFICATION_SUBJECT_SCHEMA: &str =
    "cindx.agent.delivery-verification-subject.v1";
pub const DELIVERY_VERIFICATION_VERDICT_SCHEMA: &str =
    "cindx.agent.delivery-verification-verdict.v1";
pub const MAX_DELIVERY_VERIFICATION_FINDINGS: usize = 8;
pub const MAX_DELIVERY_VERIFICATION_FINDING_SUMMARY_BYTES: usize = 512;
pub const MAX_DELIVERY_VERIFICATION_VERDICT_BYTES: usize = 8 * 1024;
pub const MAX_DELIVERY_VERIFICATION_BOUND_CONTEXT_BYTES: usize = 256 * 1024;

const SUBJECT_DIGEST_DOMAIN: &str = "cindx.agent.delivery-verification-subject-digest.v1";
const VERDICT_DIGEST_DOMAIN: &str = "cindx.agent.delivery-verification-verdict-digest.v1";
const BOUND_CONTEXT_DIGEST_DOMAIN: &str =
    "cindx.agent.delivery-verification-bound-context-digest.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryVerificationIssue {
    EmptyObjective,
    EmptyCandidate,
    InvalidGroundedReceipt,
    CandidateBindingMismatch,
    EmptyReferenceContent,
    BoundContextTooLarge,
    BoundContextMismatch,
    InvalidSubject,
    VerdictTooLarge,
    InvalidVerdictJson,
    VerdictBindingMismatch,
    MissingReviewCoverage,
    InvalidFindings,
    InvalidTransition,
}

impl fmt::Display for DeliveryVerificationIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyObjective => "delivery verification objective is empty",
            Self::EmptyCandidate => "delivery verification candidate is empty",
            Self::InvalidGroundedReceipt => "grounded completion receipt is invalid",
            Self::CandidateBindingMismatch => {
                "delivery verification candidate does not match its grounded receipt"
            }
            Self::EmptyReferenceContent => "delivery verification reference content is empty",
            Self::BoundContextTooLarge => {
                "delivery verification reference content exceeds its size bound"
            }
            Self::BoundContextMismatch => {
                "delivery verification reference content does not match its subject"
            }
            Self::InvalidSubject => "delivery verification subject is invalid",
            Self::VerdictTooLarge => "delivery verification verdict exceeds its size bound",
            Self::InvalidVerdictJson => "delivery verification verdict JSON is invalid",
            Self::VerdictBindingMismatch => {
                "delivery verification verdict does not match its subject"
            }
            Self::MissingReviewCoverage => {
                "delivery verification verdict does not cover the bound references"
            }
            Self::InvalidFindings => "delivery verification findings are invalid",
            Self::InvalidTransition => "delivery verification transition is invalid",
        })
    }
}

impl std::error::Error for DeliveryVerificationIssue {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryVerificationObligation {
    pub obligation_ref: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryVerificationEvidence {
    pub evidence_ref: u64,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryVerificationSubjectV1 {
    pub schema: String,
    pub steer_epoch: u64,
    pub contract_epoch: u64,
    pub model_turn: u64,
    pub objective_sha256: String,
    pub candidate_sha256: String,
    pub candidate_bytes: u64,
    pub obligation_digest: String,
    pub obligation_refs: Vec<String>,
    pub evidence_refs: Vec<u64>,
    pub bound_context_sha256: String,
    pub constraint_codes: Vec<String>,
    pub grounded_basis: GroundedCompletionBasis,
    pub subject_sha256: String,
}

impl DeliveryVerificationSubjectV1 {
    pub fn bind(
        objective: &str,
        candidate: &str,
        receipt: &GroundedCompletionReceipt,
        obligations: &[DeliveryVerificationObligation],
        evidence: &[DeliveryVerificationEvidence],
    ) -> Result<Self, DeliveryVerificationIssue> {
        if objective.trim().is_empty() {
            return Err(DeliveryVerificationIssue::EmptyObjective);
        }
        if candidate.trim().is_empty() {
            return Err(DeliveryVerificationIssue::EmptyCandidate);
        }
        if !receipt.contract_is_valid() {
            return Err(DeliveryVerificationIssue::InvalidGroundedReceipt);
        }

        let candidate_sha256 = sha256_hex(candidate.as_bytes());
        let candidate_bytes = u64::try_from(candidate.len()).unwrap_or(u64::MAX);
        if receipt.content_sha256 != candidate_sha256 || receipt.content_bytes != candidate_bytes {
            return Err(DeliveryVerificationIssue::CandidateBindingMismatch);
        }
        validate_reference_context(receipt, obligations, evidence)?;
        let bound_context_sha256 = reference_context_digest(obligations, evidence)
            .ok_or(DeliveryVerificationIssue::InvalidSubject)?;

        let mut subject = Self {
            schema: DELIVERY_VERIFICATION_SUBJECT_SCHEMA.to_string(),
            steer_epoch: receipt.steer_epoch,
            contract_epoch: receipt.contract_epoch,
            model_turn: receipt.model_turn,
            objective_sha256: sha256_hex(objective.as_bytes()),
            candidate_sha256,
            candidate_bytes,
            obligation_digest: receipt.obligation_digest.clone(),
            obligation_refs: receipt.covered_obligation_ids.clone(),
            evidence_refs: receipt.visible_evidence_sequences.clone(),
            bound_context_sha256,
            constraint_codes: receipt.constraint_codes.clone(),
            grounded_basis: receipt.basis,
            subject_sha256: String::new(),
        };
        subject.subject_sha256 =
            subject_digest(&subject).ok_or(DeliveryVerificationIssue::InvalidSubject)?;
        subject
            .contract_is_valid()
            .then_some(subject)
            .ok_or(DeliveryVerificationIssue::InvalidSubject)
    }

    pub fn contract_is_valid(&self) -> bool {
        self.schema == DELIVERY_VERIFICATION_SUBJECT_SCHEMA
            && self.candidate_bytes > 0
            && self.contract_epoch <= self.steer_epoch
            && is_sha256_hex(&self.objective_sha256)
            && is_sha256_hex(&self.candidate_sha256)
            && is_sha256_hex(&self.obligation_digest)
            && is_sha256_hex(&self.bound_context_sha256)
            && is_sha256_hex(&self.subject_sha256)
            && strictly_increasing_strings(&self.obligation_refs)
            && self.obligation_refs.iter().all(|item| is_sha256_hex(item))
            && strictly_increasing(self.evidence_refs.iter().copied())
            && strictly_increasing_strings(&self.constraint_codes)
            && subject_digest(self).is_some_and(|digest| digest == self.subject_sha256)
    }

    pub fn binds_reference_context(
        &self,
        obligations: &[DeliveryVerificationObligation],
        evidence: &[DeliveryVerificationEvidence],
    ) -> bool {
        self.contract_is_valid()
            && obligations
                .iter()
                .map(|item| item.obligation_ref.as_str())
                .eq(self.obligation_refs.iter().map(String::as_str))
            && evidence
                .iter()
                .map(|item| item.evidence_ref)
                .eq(self.evidence_refs.iter().copied())
            && reference_content_bytes(obligations, evidence).is_ok()
            && reference_context_digest(obligations, evidence)
                .is_some_and(|digest| digest == self.bound_context_sha256)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryVerificationDecision {
    Passed,
    NeedsRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryVerificationFindingKind {
    UnsupportedClaim,
    OmittedObligation,
    Contradiction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryVerificationFinding {
    pub kind: DeliveryVerificationFindingKind,
    pub summary: String,
    #[serde(default)]
    pub obligation_refs: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryVerificationVerdictV1 {
    pub schema: String,
    pub subject_sha256: String,
    pub reviewed_objective_sha256: String,
    pub decision: DeliveryVerificationDecision,
    pub reviewed_obligation_refs: Vec<String>,
    pub reviewed_evidence_refs: Vec<u64>,
    pub findings: Vec<DeliveryVerificationFinding>,
    pub receipt_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeliveryVerificationVerdictWire {
    schema: String,
    subject_sha256: String,
    reviewed_objective_sha256: String,
    decision: DeliveryVerificationDecision,
    reviewed_obligation_refs: Vec<String>,
    reviewed_evidence_refs: Vec<u64>,
    #[serde(default)]
    findings: Vec<DeliveryVerificationFinding>,
}

impl DeliveryVerificationVerdictV1 {
    pub fn from_json(
        subject: &DeliveryVerificationSubjectV1,
        json: &str,
    ) -> Result<Self, DeliveryVerificationIssue> {
        if !subject.contract_is_valid() {
            return Err(DeliveryVerificationIssue::InvalidSubject);
        }
        if json.len() > MAX_DELIVERY_VERIFICATION_VERDICT_BYTES {
            return Err(DeliveryVerificationIssue::VerdictTooLarge);
        }
        let wire = serde_json::from_str::<DeliveryVerificationVerdictWire>(json)
            .map_err(|_| DeliveryVerificationIssue::InvalidVerdictJson)?;

        if wire.schema != DELIVERY_VERIFICATION_VERDICT_SCHEMA
            || wire.subject_sha256 != subject.subject_sha256
            || wire.reviewed_objective_sha256 != subject.objective_sha256
        {
            return Err(DeliveryVerificationIssue::VerdictBindingMismatch);
        }
        if wire.reviewed_obligation_refs != subject.obligation_refs
            || wire.reviewed_evidence_refs != subject.evidence_refs
        {
            return Err(DeliveryVerificationIssue::MissingReviewCoverage);
        }
        validate_findings(subject, wire.decision, &wire.findings)?;

        let mut verdict = Self {
            schema: wire.schema,
            subject_sha256: wire.subject_sha256,
            reviewed_objective_sha256: wire.reviewed_objective_sha256,
            decision: wire.decision,
            reviewed_obligation_refs: wire.reviewed_obligation_refs,
            reviewed_evidence_refs: wire.reviewed_evidence_refs,
            findings: wire.findings,
            receipt_sha256: String::new(),
        };
        verdict.receipt_sha256 =
            verdict_digest(&verdict).ok_or(DeliveryVerificationIssue::InvalidVerdictJson)?;
        Ok(verdict)
    }

    pub fn contract_is_valid_for(&self, subject: &DeliveryVerificationSubjectV1) -> bool {
        subject.contract_is_valid()
            && self.schema == DELIVERY_VERIFICATION_VERDICT_SCHEMA
            && self.subject_sha256 == subject.subject_sha256
            && self.reviewed_objective_sha256 == subject.objective_sha256
            && self.reviewed_obligation_refs == subject.obligation_refs
            && self.reviewed_evidence_refs == subject.evidence_refs
            && is_sha256_hex(&self.receipt_sha256)
            && validate_findings(subject, self.decision, &self.findings).is_ok()
            && verdict_digest(self).is_some_and(|digest| digest == self.receipt_sha256)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryVerificationStatus {
    AwaitingVerification,
    AwaitingRepair,
    AwaitingRecheck,
    Passed,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryVerificationStateV1 {
    initial_subject: DeliveryVerificationSubjectV1,
    initial_verdict: Option<DeliveryVerificationVerdictV1>,
    repaired_subject: Option<DeliveryVerificationSubjectV1>,
    recheck_verdict: Option<DeliveryVerificationVerdictV1>,
    status: DeliveryVerificationStatus,
}

impl DeliveryVerificationStateV1 {
    pub fn new(subject: DeliveryVerificationSubjectV1) -> Result<Self, DeliveryVerificationIssue> {
        if !subject.contract_is_valid() {
            return Err(DeliveryVerificationIssue::InvalidSubject);
        }
        Ok(Self {
            initial_subject: subject,
            initial_verdict: None,
            repaired_subject: None,
            recheck_verdict: None,
            status: DeliveryVerificationStatus::AwaitingVerification,
        })
    }

    pub fn status(&self) -> DeliveryVerificationStatus {
        self.status
    }

    pub fn subject(&self) -> &DeliveryVerificationSubjectV1 {
        self.repaired_subject
            .as_ref()
            .unwrap_or(&self.initial_subject)
    }

    pub fn initial_verdict(&self) -> Option<&DeliveryVerificationVerdictV1> {
        self.initial_verdict.as_ref()
    }

    pub fn repaired_subject(&self) -> Option<&DeliveryVerificationSubjectV1> {
        self.repaired_subject.as_ref()
    }

    pub fn recheck_verdict(&self) -> Option<&DeliveryVerificationVerdictV1> {
        self.recheck_verdict.as_ref()
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            DeliveryVerificationStatus::Passed | DeliveryVerificationStatus::Unverified
        )
    }

    pub fn record_initial_verdict(
        &mut self,
        verdict: DeliveryVerificationVerdictV1,
    ) -> Result<(), DeliveryVerificationIssue> {
        if self.status != DeliveryVerificationStatus::AwaitingVerification
            || !verdict.contract_is_valid_for(&self.initial_subject)
        {
            return Err(DeliveryVerificationIssue::InvalidTransition);
        }
        self.status = match verdict.decision {
            DeliveryVerificationDecision::Passed => DeliveryVerificationStatus::Passed,
            DeliveryVerificationDecision::NeedsRevision => {
                DeliveryVerificationStatus::AwaitingRepair
            }
        };
        self.initial_verdict = Some(verdict);
        Ok(())
    }

    pub fn record_repair(
        &mut self,
        repaired_subject: DeliveryVerificationSubjectV1,
    ) -> Result<(), DeliveryVerificationIssue> {
        if self.status != DeliveryVerificationStatus::AwaitingRepair
            || !repair_preserves_scope(&self.initial_subject, &repaired_subject)
        {
            return Err(DeliveryVerificationIssue::InvalidTransition);
        }
        self.repaired_subject = Some(repaired_subject);
        self.status = DeliveryVerificationStatus::AwaitingRecheck;
        Ok(())
    }

    pub fn record_recheck(
        &mut self,
        verdict: DeliveryVerificationVerdictV1,
    ) -> Result<(), DeliveryVerificationIssue> {
        let Some(repaired_subject) = self.repaired_subject.as_ref() else {
            return Err(DeliveryVerificationIssue::InvalidTransition);
        };
        if self.status != DeliveryVerificationStatus::AwaitingRecheck
            || !verdict.contract_is_valid_for(repaired_subject)
        {
            return Err(DeliveryVerificationIssue::InvalidTransition);
        }
        self.status = match verdict.decision {
            DeliveryVerificationDecision::Passed => DeliveryVerificationStatus::Passed,
            DeliveryVerificationDecision::NeedsRevision => DeliveryVerificationStatus::Unverified,
        };
        self.recheck_verdict = Some(verdict);
        Ok(())
    }

    pub fn fail_closed(&mut self) -> Result<(), DeliveryVerificationIssue> {
        if self.is_terminal() {
            return Err(DeliveryVerificationIssue::InvalidTransition);
        }
        self.status = DeliveryVerificationStatus::Unverified;
        Ok(())
    }
}

impl DeliveryVerificationStatus {
    /// The wire label a run records on its terminal metadata.
    pub fn label(self) -> &'static str {
        match self {
            Self::AwaitingVerification => "awaiting_verification",
            Self::AwaitingRepair => "awaiting_repair",
            Self::AwaitingRecheck => "awaiting_recheck",
            Self::Passed => "passed",
            Self::Unverified => "unverified",
        }
    }
}

fn repair_preserves_scope(
    initial: &DeliveryVerificationSubjectV1,
    repaired: &DeliveryVerificationSubjectV1,
) -> bool {
    repaired.contract_is_valid()
        && repaired.candidate_sha256 != initial.candidate_sha256
        && repaired.steer_epoch == initial.steer_epoch
        && repaired.contract_epoch == initial.contract_epoch
        && repaired.model_turn > initial.model_turn
        && repaired.objective_sha256 == initial.objective_sha256
        && repaired.obligation_digest == initial.obligation_digest
        && repaired.obligation_refs == initial.obligation_refs
        && repaired.evidence_refs == initial.evidence_refs
        && repaired.bound_context_sha256 == initial.bound_context_sha256
        && repaired.constraint_codes == initial.constraint_codes
        && repaired.grounded_basis == initial.grounded_basis
}

fn validate_findings(
    subject: &DeliveryVerificationSubjectV1,
    decision: DeliveryVerificationDecision,
    findings: &[DeliveryVerificationFinding],
) -> Result<(), DeliveryVerificationIssue> {
    if findings.len() > MAX_DELIVERY_VERIFICATION_FINDINGS
        || match decision {
            DeliveryVerificationDecision::Passed => !findings.is_empty(),
            DeliveryVerificationDecision::NeedsRevision => findings.is_empty(),
        }
    {
        return Err(DeliveryVerificationIssue::InvalidFindings);
    }

    for (index, finding) in findings.iter().enumerate() {
        if finding.summary.trim().is_empty()
            || finding.summary.len() > MAX_DELIVERY_VERIFICATION_FINDING_SUMMARY_BYTES
            || finding.summary.chars().any(char::is_control)
            || !strictly_increasing_strings(&finding.obligation_refs)
            || !strictly_increasing(finding.evidence_refs.iter().copied())
            || finding
                .obligation_refs
                .iter()
                .any(|reference| subject.obligation_refs.binary_search(reference).is_err())
            || finding
                .evidence_refs
                .iter()
                .any(|reference| subject.evidence_refs.binary_search(reference).is_err())
            || findings[..index].contains(finding)
        {
            return Err(DeliveryVerificationIssue::InvalidFindings);
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubjectDigestPayload<'a> {
    domain: &'static str,
    schema: &'a str,
    steer_epoch: u64,
    contract_epoch: u64,
    model_turn: u64,
    objective_sha256: &'a str,
    candidate_sha256: &'a str,
    candidate_bytes: u64,
    obligation_digest: &'a str,
    obligation_refs: &'a [String],
    evidence_refs: &'a [u64],
    bound_context_sha256: &'a str,
    constraint_codes: &'a [String],
    grounded_basis: GroundedCompletionBasis,
}

fn subject_digest(subject: &DeliveryVerificationSubjectV1) -> Option<String> {
    digest_json(&SubjectDigestPayload {
        domain: SUBJECT_DIGEST_DOMAIN,
        schema: &subject.schema,
        steer_epoch: subject.steer_epoch,
        contract_epoch: subject.contract_epoch,
        model_turn: subject.model_turn,
        objective_sha256: &subject.objective_sha256,
        candidate_sha256: &subject.candidate_sha256,
        candidate_bytes: subject.candidate_bytes,
        obligation_digest: &subject.obligation_digest,
        obligation_refs: &subject.obligation_refs,
        evidence_refs: &subject.evidence_refs,
        bound_context_sha256: &subject.bound_context_sha256,
        constraint_codes: &subject.constraint_codes,
        grounded_basis: subject.grounded_basis,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BoundContextDigestPayload<'a> {
    domain: &'static str,
    obligations: &'a [DeliveryVerificationObligation],
    evidence: &'a [DeliveryVerificationEvidence],
}

fn reference_context_digest(
    obligations: &[DeliveryVerificationObligation],
    evidence: &[DeliveryVerificationEvidence],
) -> Option<String> {
    digest_json(&BoundContextDigestPayload {
        domain: BOUND_CONTEXT_DIGEST_DOMAIN,
        obligations,
        evidence,
    })
}

fn validate_reference_context(
    receipt: &GroundedCompletionReceipt,
    obligations: &[DeliveryVerificationObligation],
    evidence: &[DeliveryVerificationEvidence],
) -> Result<(), DeliveryVerificationIssue> {
    if obligations
        .iter()
        .map(|item| item.obligation_ref.as_str())
        .ne(receipt.covered_obligation_ids.iter().map(String::as_str))
        || evidence
            .iter()
            .map(|item| item.evidence_ref)
            .ne(receipt.visible_evidence_sequences.iter().copied())
    {
        return Err(DeliveryVerificationIssue::BoundContextMismatch);
    }
    reference_content_bytes(obligations, evidence).map(|_| ())
}

fn reference_content_bytes(
    obligations: &[DeliveryVerificationObligation],
    evidence: &[DeliveryVerificationEvidence],
) -> Result<usize, DeliveryVerificationIssue> {
    let mut content_bytes = 0usize;
    for content in obligations
        .iter()
        .map(|item| item.content.as_str())
        .chain(evidence.iter().map(|item| item.content.as_str()))
    {
        if content.trim().is_empty() {
            return Err(DeliveryVerificationIssue::EmptyReferenceContent);
        }
        content_bytes = content_bytes
            .checked_add(content.len())
            .ok_or(DeliveryVerificationIssue::BoundContextTooLarge)?;
    }
    if content_bytes > MAX_DELIVERY_VERIFICATION_BOUND_CONTEXT_BYTES {
        return Err(DeliveryVerificationIssue::BoundContextTooLarge);
    }
    Ok(content_bytes)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerdictDigestPayload<'a> {
    domain: &'static str,
    schema: &'a str,
    subject_sha256: &'a str,
    reviewed_objective_sha256: &'a str,
    decision: DeliveryVerificationDecision,
    reviewed_obligation_refs: &'a [String],
    reviewed_evidence_refs: &'a [u64],
    findings: &'a [DeliveryVerificationFinding],
}

fn verdict_digest(verdict: &DeliveryVerificationVerdictV1) -> Option<String> {
    digest_json(&VerdictDigestPayload {
        domain: VERDICT_DIGEST_DOMAIN,
        schema: &verdict.schema,
        subject_sha256: &verdict.subject_sha256,
        reviewed_objective_sha256: &verdict.reviewed_objective_sha256,
        decision: verdict.decision,
        reviewed_obligation_refs: &verdict.reviewed_obligation_refs,
        reviewed_evidence_refs: &verdict.reviewed_evidence_refs,
        findings: &verdict.findings,
    })
}

fn digest_json(value: &impl Serialize) -> Option<String> {
    serde_json::to_vec(value)
        .ok()
        .map(|bytes| sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn strictly_increasing<T>(items: impl IntoIterator<Item = T>) -> bool
where
    T: Ord,
{
    let mut previous = None;
    for item in items {
        if previous.as_ref().is_some_and(|prior| prior >= &item) {
            return false;
        }
        previous = Some(item);
    }
    true
}

fn strictly_increasing_strings(items: &[String]) -> bool {
    strictly_increasing(items.iter())
}

/// Turns one finding summary into the bounded, control-free text this contract
/// accepts: at most [`MAX_DELIVERY_VERIFICATION_FINDING_SUMMARY_BYTES`] bytes,
/// cut on a character boundary.
fn bounded_finding_summary(summary: &str) -> String {
    let mut cleaned: String = summary
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    while cleaned.len() > MAX_DELIVERY_VERIFICATION_FINDING_SUMMARY_BYTES {
        let cut = cleaned
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= MAX_DELIVERY_VERIFICATION_FINDING_SUMMARY_BYTES)
            .last()
            .unwrap_or(0);
        cleaned.truncate(cut);
    }
    cleaned.trim().to_string()
}

/// The deterministic verdict producer for this contract: it classifies a
/// claim-evidence receipt into the typed finding vocabulary with no provider call.
///
/// A contradicted citation becomes
/// [`DeliveryVerificationFindingKind::Contradiction`] and an unsupported one
/// becomes [`DeliveryVerificationFindingKind::UnsupportedClaim`]. A receipt with no
/// findings — including an answer that cites no workspace location at all —
/// yields `Passed` with the empty finding list the contract requires.
/// [`DeliveryVerificationFindingKind::OmittedObligation`] is deliberately never
/// produced here: deciding that an answer omitted an obligation means reading the
/// obligations, which a location-level binder does not do. Findings carry no
/// obligation or evidence refs for the same reason — a ref would claim a binding
/// this producer did not check.
pub fn claim_evidence_verdict(
    subject: &DeliveryVerificationSubjectV1,
    claims: &ClaimEvidenceReceipt,
) -> Result<DeliveryVerificationVerdictV1, DeliveryVerificationIssue> {
    if !subject.contract_is_valid() {
        return Err(DeliveryVerificationIssue::InvalidSubject);
    }
    // The claim receipt must have been computed over exactly the candidate this
    // subject binds, so a receipt for one answer can never verify another.
    if claims.answer_sha256 != subject.candidate_sha256 {
        return Err(DeliveryVerificationIssue::VerdictBindingMismatch);
    }
    let findings = claims
        .findings
        .iter()
        .take(MAX_DELIVERY_VERIFICATION_FINDINGS)
        .map(|finding| DeliveryVerificationFinding {
            kind: match finding.status {
                CitationStatus::Contradicted => DeliveryVerificationFindingKind::Contradiction,
                CitationStatus::Supported | CitationStatus::Unsupported => {
                    DeliveryVerificationFindingKind::UnsupportedClaim
                }
            },
            summary: bounded_finding_summary(&format!(
                "{} citation `{}`: {}",
                finding.status.label(),
                finding.citation.display(),
                finding.reason
            )),
            obligation_refs: Vec::new(),
            evidence_refs: Vec::new(),
        })
        .collect::<Vec<_>>();
    let decision = if findings.is_empty() {
        DeliveryVerificationDecision::Passed
    } else {
        DeliveryVerificationDecision::NeedsRevision
    };
    let mut verdict = DeliveryVerificationVerdictV1 {
        schema: DELIVERY_VERIFICATION_VERDICT_SCHEMA.to_string(),
        subject_sha256: subject.subject_sha256.clone(),
        reviewed_objective_sha256: subject.objective_sha256.clone(),
        decision,
        reviewed_obligation_refs: subject.obligation_refs.clone(),
        reviewed_evidence_refs: subject.evidence_refs.clone(),
        findings,
        receipt_sha256: String::new(),
    };
    validate_findings(subject, verdict.decision, &verdict.findings)?;
    verdict.receipt_sha256 =
        verdict_digest(&verdict).ok_or(DeliveryVerificationIssue::InvalidVerdictJson)?;
    verdict
        .contract_is_valid_for(subject)
        .then_some(verdict)
        .ok_or(DeliveryVerificationIssue::InvalidVerdictJson)
}

/// Binds the subject to the exact candidate bytes and reference context, records
/// the deterministic claim verdict, and closes the state.
///
/// A `Passed` verdict terminates as `Passed`. A `NeedsRevision` verdict terminates
/// as `Unverified` rather than resting in `AwaitingRepair`, because this contract
/// attempts no repair of its own: the product's judge gate owns repair, and
/// leaving the state open would report an unrepaired answer as still under review
/// after the run has committed.
pub fn verify_delivery_against_claims(
    objective: &str,
    candidate: &str,
    receipt: &GroundedCompletionReceipt,
    obligations: &[DeliveryVerificationObligation],
    evidence: &[DeliveryVerificationEvidence],
    claims: &ClaimEvidenceReceipt,
) -> Result<DeliveryVerificationStateV1, DeliveryVerificationIssue> {
    let subject =
        DeliveryVerificationSubjectV1::bind(objective, candidate, receipt, obligations, evidence)?;
    let verdict = claim_evidence_verdict(&subject, claims)?;
    let mut state = DeliveryVerificationStateV1::new(subject)?;
    state.record_initial_verdict(verdict)?;
    if !state.is_terminal() {
        state.fail_closed()?;
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claim_evidence::{bind_answer_citations, ObservedLocation, ObservedLocationKind};
    use crate::GROUNDED_COMPLETION_SCHEMA;
    use serde_json::{json, Value};

    const OBJECTIVE: &str = "Return an evidence-grounded answer.";
    const CANDIDATE: &str = "The bounded evidence supports the answer.";
    const REPAIRED_CANDIDATE: &str = "The bounded evidence supports the corrected answer.";

    fn grounded_receipt(candidate: &str) -> GroundedCompletionReceipt {
        GroundedCompletionReceipt {
            schema: GROUNDED_COMPLETION_SCHEMA.to_string(),
            steer_epoch: 4,
            contract_epoch: 3,
            model_turn: 2,
            content_sha256: sha256_hex(candidate.as_bytes()),
            content_bytes: candidate.len() as u64,
            obligation_digest: "1".repeat(64),
            covered_obligation_ids: vec!["2".repeat(64), "3".repeat(64)],
            visible_evidence_sequences: vec![7, 9],
            constraint_codes: Vec::new(),
            basis: GroundedCompletionBasis::EvidenceVisible,
        }
    }

    fn obligations() -> Vec<DeliveryVerificationObligation> {
        vec![
            DeliveryVerificationObligation {
                obligation_ref: "2".repeat(64),
                content: "State the evidence-supported result.".to_string(),
            },
            DeliveryVerificationObligation {
                obligation_ref: "3".repeat(64),
                content: "Do not add unsupported claims.".to_string(),
            },
        ]
    }

    fn evidence() -> Vec<DeliveryVerificationEvidence> {
        vec![
            DeliveryVerificationEvidence {
                evidence_ref: 7,
                content: "The bounded evidence supports the answer.".to_string(),
            },
            DeliveryVerificationEvidence {
                evidence_ref: 9,
                content: "No conflicting evidence was supplied.".to_string(),
            },
        ]
    }

    fn subject_at(candidate: &str, model_turn: u64) -> DeliveryVerificationSubjectV1 {
        let mut receipt = grounded_receipt(candidate);
        receipt.model_turn = model_turn;
        DeliveryVerificationSubjectV1::bind(
            OBJECTIVE,
            candidate,
            &receipt,
            &obligations(),
            &evidence(),
        )
        .expect("fixture subject should be valid")
    }

    fn subject(candidate: &str) -> DeliveryVerificationSubjectV1 {
        subject_at(candidate, 2)
    }

    fn finding() -> Value {
        json!({
            "kind": "unsupported_claim",
            "summary": "One claim is not supported by the bound evidence.",
            "obligationRefs": ["2".repeat(64)],
            "evidenceRefs": [7]
        })
    }

    fn verdict_value(
        subject: &DeliveryVerificationSubjectV1,
        decision: &str,
        findings: Vec<Value>,
    ) -> Value {
        json!({
            "schema": DELIVERY_VERIFICATION_VERDICT_SCHEMA,
            "subjectSha256": subject.subject_sha256,
            "reviewedObjectiveSha256": subject.objective_sha256,
            "decision": decision,
            "reviewedObligationRefs": subject.obligation_refs,
            "reviewedEvidenceRefs": subject.evidence_refs,
            "findings": findings
        })
    }

    fn verdict(
        subject: &DeliveryVerificationSubjectV1,
        decision: &str,
    ) -> DeliveryVerificationVerdictV1 {
        let findings = if decision == "passed" {
            Vec::new()
        } else {
            vec![finding()]
        };
        DeliveryVerificationVerdictV1::from_json(
            subject,
            &verdict_value(subject, decision, findings).to_string(),
        )
        .expect("fixture verdict should be valid")
    }

    #[test]
    fn delivery_verification_contract_binds_exact_objective_candidate_and_receipt() {
        println!("{DELIVERY_VERIFICATION_SUBJECT_SCHEMA}");
        let subject = subject(CANDIDATE);

        assert!(subject.contract_is_valid());
        assert_eq!(subject.objective_sha256, sha256_hex(OBJECTIVE.as_bytes()));
        assert_eq!(subject.candidate_sha256, sha256_hex(CANDIDATE.as_bytes()));
        assert_eq!(subject.candidate_bytes, CANDIDATE.len() as u64);
        assert_eq!(
            subject.obligation_refs,
            vec!["2".repeat(64), "3".repeat(64)]
        );
        assert_eq!(subject.evidence_refs, vec![7, 9]);
        assert_eq!(
            DeliveryVerificationSubjectV1::bind(
                OBJECTIVE,
                "different candidate",
                &grounded_receipt(CANDIDATE),
                &obligations(),
                &evidence(),
            ),
            Err(DeliveryVerificationIssue::CandidateBindingMismatch)
        );
    }

    #[test]
    fn delivery_verification_contract_rejects_empty_inputs_and_invalid_receipt() {
        assert_eq!(
            DeliveryVerificationSubjectV1::bind(
                "  ",
                CANDIDATE,
                &grounded_receipt(CANDIDATE),
                &obligations(),
                &evidence(),
            ),
            Err(DeliveryVerificationIssue::EmptyObjective)
        );
        assert_eq!(
            DeliveryVerificationSubjectV1::bind(
                OBJECTIVE,
                "\n",
                &grounded_receipt(CANDIDATE),
                &obligations(),
                &evidence(),
            ),
            Err(DeliveryVerificationIssue::EmptyCandidate)
        );
        let mut receipt = grounded_receipt(CANDIDATE);
        receipt.schema = "wrong".to_string();
        assert_eq!(
            DeliveryVerificationSubjectV1::bind(
                OBJECTIVE,
                CANDIDATE,
                &receipt,
                &obligations(),
                &evidence(),
            ),
            Err(DeliveryVerificationIssue::InvalidGroundedReceipt)
        );
    }

    #[test]
    fn delivery_verification_contract_binds_exact_ordered_reference_content() {
        let subject = subject(CANDIDATE);
        let obligations = obligations();
        let evidence = evidence();
        assert!(subject.binds_reference_context(&obligations, &evidence));

        let mut changed = evidence.clone();
        changed[0].content = "The same reference now has different text.".to_string();
        assert!(!subject.binds_reference_context(&obligations, &changed));

        let mut empty = obligations.clone();
        empty[0].content = "  ".to_string();
        assert_eq!(
            DeliveryVerificationSubjectV1::bind(
                OBJECTIVE,
                CANDIDATE,
                &grounded_receipt(CANDIDATE),
                &empty,
                &evidence,
            ),
            Err(DeliveryVerificationIssue::EmptyReferenceContent)
        );

        let mut missing_evidence = evidence.clone();
        missing_evidence.pop();
        assert_eq!(
            DeliveryVerificationSubjectV1::bind(
                OBJECTIVE,
                CANDIDATE,
                &grounded_receipt(CANDIDATE),
                &obligations,
                &missing_evidence,
            ),
            Err(DeliveryVerificationIssue::BoundContextMismatch)
        );
    }

    #[test]
    fn delivery_verification_contract_parses_strict_bound_pass_verdict() {
        let subject = subject(CANDIDATE);
        let verdict = verdict(&subject, "passed");

        assert_eq!(verdict.decision, DeliveryVerificationDecision::Passed);
        assert!(verdict.contract_is_valid_for(&subject));
        assert!(is_sha256_hex(&verdict.receipt_sha256));
    }

    #[test]
    fn delivery_verification_contract_rejects_non_json_unknown_and_oversized_verdicts() {
        let subject = subject(CANDIDATE);
        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(&subject, "```json\n{}\n```"),
            Err(DeliveryVerificationIssue::InvalidVerdictJson)
        );

        let mut unknown = verdict_value(&subject, "passed", Vec::new());
        unknown["extra"] = json!(true);
        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(&subject, &unknown.to_string()),
            Err(DeliveryVerificationIssue::InvalidVerdictJson)
        );

        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(
                &subject,
                &"x".repeat(MAX_DELIVERY_VERIFICATION_VERDICT_BYTES + 1)
            ),
            Err(DeliveryVerificationIssue::VerdictTooLarge)
        );
    }

    #[test]
    fn delivery_verification_contract_rejects_wrong_binding_or_incomplete_coverage() {
        let subject = subject(CANDIDATE);
        let mut wrong_binding = verdict_value(&subject, "passed", Vec::new());
        wrong_binding["subjectSha256"] = json!("f".repeat(64));
        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(&subject, &wrong_binding.to_string()),
            Err(DeliveryVerificationIssue::VerdictBindingMismatch)
        );

        let mut incomplete = verdict_value(&subject, "passed", Vec::new());
        incomplete["reviewedEvidenceRefs"] = json!([7]);
        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(&subject, &incomplete.to_string()),
            Err(DeliveryVerificationIssue::MissingReviewCoverage)
        );
    }

    #[test]
    fn delivery_verification_contract_enforces_bounded_consistent_findings() {
        let subject = subject(CANDIDATE);
        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(
                &subject,
                &verdict_value(&subject, "needs_revision", Vec::new()).to_string()
            ),
            Err(DeliveryVerificationIssue::InvalidFindings)
        );
        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(
                &subject,
                &verdict_value(&subject, "passed", vec![finding()]).to_string()
            ),
            Err(DeliveryVerificationIssue::InvalidFindings)
        );

        let mut unknown_reference = finding();
        unknown_reference["evidenceRefs"] = json!([8]);
        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(
                &subject,
                &verdict_value(&subject, "needs_revision", vec![unknown_reference]).to_string()
            ),
            Err(DeliveryVerificationIssue::InvalidFindings)
        );

        let too_many = vec![finding(); MAX_DELIVERY_VERIFICATION_FINDINGS + 1];
        assert_eq!(
            DeliveryVerificationVerdictV1::from_json(
                &subject,
                &verdict_value(&subject, "needs_revision", too_many).to_string()
            ),
            Err(DeliveryVerificationIssue::InvalidFindings)
        );
    }

    #[test]
    fn delivery_verification_contract_initial_pass_is_terminal() {
        let subject = subject(CANDIDATE);
        let mut state = DeliveryVerificationStateV1::new(subject.clone()).unwrap();

        state
            .record_initial_verdict(verdict(&subject, "passed"))
            .unwrap();
        assert_eq!(state.status(), DeliveryVerificationStatus::Passed);
        assert!(state.is_terminal());
        assert_eq!(
            state.fail_closed(),
            Err(DeliveryVerificationIssue::InvalidTransition)
        );
    }

    #[test]
    fn delivery_verification_contract_allows_one_repair_and_one_passing_recheck() {
        let initial = subject(CANDIDATE);
        let repaired = subject_at(REPAIRED_CANDIDATE, 3);
        let mut state = DeliveryVerificationStateV1::new(initial.clone()).unwrap();

        state
            .record_initial_verdict(verdict(&initial, "needs_revision"))
            .unwrap();
        assert_eq!(state.status(), DeliveryVerificationStatus::AwaitingRepair);
        state.record_repair(repaired.clone()).unwrap();
        assert_eq!(state.status(), DeliveryVerificationStatus::AwaitingRecheck);
        state.record_recheck(verdict(&repaired, "passed")).unwrap();

        assert_eq!(state.status(), DeliveryVerificationStatus::Passed);
        assert_eq!(state.subject(), &repaired);
        assert!(state.initial_verdict().is_some());
        assert!(state.repaired_subject().is_some());
        assert!(state.recheck_verdict().is_some());
    }

    #[test]
    fn delivery_verification_contract_failed_recheck_is_terminal_and_cannot_loop() {
        let initial = subject(CANDIDATE);
        let repaired = subject_at(REPAIRED_CANDIDATE, 3);
        let mut state = DeliveryVerificationStateV1::new(initial.clone()).unwrap();

        state
            .record_initial_verdict(verdict(&initial, "needs_revision"))
            .unwrap();
        state.record_repair(repaired.clone()).unwrap();
        state
            .record_recheck(verdict(&repaired, "needs_revision"))
            .unwrap();

        assert_eq!(state.status(), DeliveryVerificationStatus::Unverified);
        assert_eq!(
            state.record_repair(subject("A third candidate is forbidden.")),
            Err(DeliveryVerificationIssue::InvalidTransition)
        );
    }

    #[test]
    fn delivery_verification_contract_repair_must_change_candidate_and_preserve_scope() {
        let initial = subject(CANDIDATE);
        let mut state = DeliveryVerificationStateV1::new(initial.clone()).unwrap();
        state
            .record_initial_verdict(verdict(&initial, "needs_revision"))
            .unwrap();

        assert_eq!(
            state.record_repair(initial.clone()),
            Err(DeliveryVerificationIssue::InvalidTransition)
        );
        let same_turn = subject(REPAIRED_CANDIDATE);
        assert_eq!(
            state.record_repair(same_turn),
            Err(DeliveryVerificationIssue::InvalidTransition)
        );

        let mut altered_evidence = evidence();
        altered_evidence[0].content =
            "Different evidence text under the same reference.".to_string();
        let mut repaired_receipt = grounded_receipt(REPAIRED_CANDIDATE);
        repaired_receipt.model_turn = 3;
        let altered_context = DeliveryVerificationSubjectV1::bind(
            OBJECTIVE,
            REPAIRED_CANDIDATE,
            &repaired_receipt,
            &obligations(),
            &altered_evidence,
        )
        .expect("altered context forms its own valid subject");
        assert_eq!(
            state.record_repair(altered_context),
            Err(DeliveryVerificationIssue::InvalidTransition)
        );

        let mut changed_scope = subject_at(REPAIRED_CANDIDATE, 3);
        changed_scope.objective_sha256 = "f".repeat(64);
        assert_eq!(
            state.record_repair(changed_scope),
            Err(DeliveryVerificationIssue::InvalidTransition)
        );
    }

    #[test]
    fn delivery_verification_contract_explicit_failure_closes_without_repair_loop() {
        let mut state = DeliveryVerificationStateV1::new(subject(CANDIDATE)).unwrap();

        state.fail_closed().unwrap();
        assert_eq!(state.status(), DeliveryVerificationStatus::Unverified);
        assert!(state.is_terminal());
        assert_eq!(
            state.fail_closed(),
            Err(DeliveryVerificationIssue::InvalidTransition)
        );
    }

    /// The deterministic producer: an answer whose citations all bind to observed
    /// locations passes the contract with the empty finding list it requires.
    #[test]
    fn a_fully_bound_answer_passes_the_delivery_verification_contract() {
        let claims = bind_answer_citations(CANDIDATE, &[]);
        assert!(claims.findings.is_empty(), "this candidate cites nothing");

        let state = verify_delivery_against_claims(
            OBJECTIVE,
            CANDIDATE,
            &grounded_receipt(CANDIDATE),
            &obligations(),
            &evidence(),
            &claims,
        )
        .expect("a claim receipt with no findings verifies");

        assert_eq!(state.status(), DeliveryVerificationStatus::Passed);
        assert!(state.is_terminal());
        let verdict = state.initial_verdict().expect("verdict recorded");
        assert_eq!(verdict.decision, DeliveryVerificationDecision::Passed);
        assert!(verdict.findings.is_empty());
        assert!(verdict.contract_is_valid_for(state.subject()));
        // Full review coverage of the bound reference context.
        assert_eq!(
            verdict.reviewed_obligation_refs,
            state.subject().obligation_refs
        );
        assert_eq!(
            verdict.reviewed_evidence_refs,
            state.subject().evidence_refs
        );
        assert_eq!(verdict.subject_sha256, state.subject().subject_sha256);
    }

    #[test]
    fn an_unsupported_citation_closes_the_contract_as_unverified_with_a_typed_finding() {
        const CITED: &str = "The fix is in src/invented.rs:12.";
        let claims = bind_answer_citations(CITED, &[]);
        assert_eq!(claims.unsupported, 1);

        let state = verify_delivery_against_claims(
            OBJECTIVE,
            CITED,
            &grounded_receipt(CITED),
            &obligations(),
            &evidence(),
            &claims,
        )
        .expect("the state closes even when the verdict needs revision");

        // No repair runs under this contract, so it closes unverified instead of
        // resting in AwaitingRepair after the run has committed.
        assert_eq!(state.status(), DeliveryVerificationStatus::Unverified);
        assert!(state.is_terminal());
        let verdict = state.initial_verdict().expect("verdict recorded");
        assert_eq!(
            verdict.decision,
            DeliveryVerificationDecision::NeedsRevision
        );
        assert_eq!(verdict.findings.len(), 1);
        assert_eq!(
            verdict.findings[0].kind,
            DeliveryVerificationFindingKind::UnsupportedClaim
        );
        assert!(
            verdict.findings[0].summary.contains("src/invented.rs:12"),
            "summary was {}",
            verdict.findings[0].summary
        );
        // A location-level binder never claims an obligation or evidence binding.
        assert!(verdict.findings[0].obligation_refs.is_empty());
        assert!(verdict.findings[0].evidence_refs.is_empty());
        assert!(verdict.contract_is_valid_for(state.subject()));
    }

    #[test]
    fn a_contradicted_citation_is_recorded_as_a_contradiction_finding() {
        const CITED: &str = "Confirmed at src/gone.rs:4.";
        let claims = bind_answer_citations(
            CITED,
            &[ObservedLocation {
                path: "src/gone.rs".to_string(),
                kind: ObservedLocationKind::Read,
                succeeded: false,
            }],
        );
        assert_eq!(claims.contradicted, 1);

        let state = verify_delivery_against_claims(
            OBJECTIVE,
            CITED,
            &grounded_receipt(CITED),
            &obligations(),
            &evidence(),
            &claims,
        )
        .expect("the state closes");

        let verdict = state.initial_verdict().expect("verdict recorded");
        assert_eq!(
            verdict.findings[0].kind,
            DeliveryVerificationFindingKind::Contradiction
        );
        assert_eq!(state.status(), DeliveryVerificationStatus::Unverified);
        // OmittedObligation is reserved for a producer that reads obligations.
        assert!(!verdict
            .findings
            .iter()
            .any(|finding| finding.kind == DeliveryVerificationFindingKind::OmittedObligation));
    }

    #[test]
    fn a_claim_receipt_for_a_different_answer_can_never_verify_this_candidate() {
        let claims = bind_answer_citations("An entirely different answer.", &[]);

        assert_eq!(
            verify_delivery_against_claims(
                OBJECTIVE,
                CANDIDATE,
                &grounded_receipt(CANDIDATE),
                &obligations(),
                &evidence(),
                &claims
            ),
            Err(DeliveryVerificationIssue::VerdictBindingMismatch)
        );
    }
}
