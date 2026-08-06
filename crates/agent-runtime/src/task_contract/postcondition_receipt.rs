mod target;
mod transition;
mod validation;

use super::{fingerprint, OutcomePostconditionKind};
use agent_core::Metadata;
use serde::{Deserialize, Serialize};

pub const POSTCONDITION_VERIFICATION_SCHEMA: &str = "cindx.postcondition-verification-receipt.v1";
pub const POSTCONDITION_VERIFICATION_METADATA_KEY: &str = "postcondition_verification_receipt";
pub const POSTCONDITION_VERIFICATION_DIGEST_METADATA_KEY: &str =
    "postcondition_verification_receipt_sha256";
pub const MAX_POSTCONDITION_VERIFICATION_RECEIPT_BYTES: usize = 2_048;
pub(super) const MAX_POSTCONDITION_VERIFICATION_RECEIPTS: usize = 32;
const MAX_VERIFIER_SOURCE_BYTES: usize = 128;
const MAX_VERIFIER_CONTRACT_BYTES: usize = 128;
const MAX_POSTCONDITION_TARGETS: usize = 8;
const POSTCONDITION_TARGET_DIGEST_DOMAIN: &str = "cindx.postcondition-target.v2";
const WORKSPACE_FILE_CONTENT_VERIFIER: &str = "workspace_file_content_v1";
const BROWSER_OBSERVATION_VERIFIER: &str = "browser_observation_v1";
const COMPUTER_OBSERVATION_VERIFIER: &str = "computer_observation_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PostconditionBindingKey {
    Workspace,
    Browser,
    Computer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PostconditionActionBinding {
    pub steer_epoch: u64,
    pub contract_epoch: u64,
    pub kind: OutcomePostconditionKind,
    pub postcondition_id: String,
    pub action_sequence: u64,
    pub action_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_contract: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_witness: Option<PostconditionTargetWitness>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered_target_digests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PostconditionTargetWitness {
    scope_digest: String,
    exact_digests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostconditionVerificationReceipt {
    pub schema: String,
    pub steer_epoch: u64,
    pub contract_epoch: u64,
    pub kind: OutcomePostconditionKind,
    pub postcondition_id: String,
    pub action_sequence: u64,
    pub observation_sequence: u64,
    pub verifier_source: String,
    pub verifier_kind: String,
    pub verifier_digest: String,
    pub receipt_digest: String,
}

impl PostconditionVerificationReceipt {
    #[allow(clippy::too_many_arguments)]
    fn new(
        steer_epoch: u64,
        contract_epoch: u64,
        kind: OutcomePostconditionKind,
        postcondition_id: String,
        action_sequence: u64,
        observation_sequence: u64,
        verifier_source: &str,
        verifier_kind: &str,
        verifier_digest: String,
    ) -> Option<Self> {
        let mut receipt = Self {
            schema: POSTCONDITION_VERIFICATION_SCHEMA.to_string(),
            steer_epoch,
            contract_epoch,
            kind,
            postcondition_id,
            action_sequence,
            observation_sequence,
            verifier_source: verifier_source.to_string(),
            verifier_kind: verifier_kind.to_string(),
            verifier_digest,
            receipt_digest: String::new(),
        };
        receipt.receipt_digest = receipt.expected_digest()?;
        receipt.contract_is_valid().then_some(receipt)
    }

    pub fn contract_is_valid(&self) -> bool {
        self.schema == POSTCONDITION_VERIFICATION_SCHEMA
            && self.contract_epoch <= self.steer_epoch
            && self.action_sequence > 0
            && self.observation_sequence > self.action_sequence
            && !self.verifier_source.trim().is_empty()
            && self.verifier_source.len() <= MAX_VERIFIER_SOURCE_BYTES
            && verifier_kind_matches_postcondition(&self.verifier_kind, self.kind)
            && is_sha256_hex(&self.postcondition_id)
            && is_sha256_hex(&self.verifier_digest)
            && is_sha256_hex(&self.receipt_digest)
            && self
                .expected_digest()
                .is_some_and(|digest| digest == self.receipt_digest)
            && serde_json::to_vec(self)
                .is_ok_and(|encoded| encoded.len() <= MAX_POSTCONDITION_VERIFICATION_RECEIPT_BYTES)
    }

    pub fn insert_metadata(&self, metadata: &mut Metadata) -> bool {
        if !self.contract_is_valid() {
            return false;
        }
        let Ok(encoded) = serde_json::to_string(self) else {
            return false;
        };
        if encoded.len() > MAX_POSTCONDITION_VERIFICATION_RECEIPT_BYTES {
            return false;
        }
        metadata.insert(
            POSTCONDITION_VERIFICATION_DIGEST_METADATA_KEY.to_string(),
            fingerprint(&encoded),
        );
        metadata.insert(POSTCONDITION_VERIFICATION_METADATA_KEY.to_string(), encoded);
        true
    }

    pub fn from_metadata(metadata: &Metadata) -> Option<Self> {
        let encoded = metadata.get(POSTCONDITION_VERIFICATION_METADATA_KEY)?;
        let digest = metadata.get(POSTCONDITION_VERIFICATION_DIGEST_METADATA_KEY)?;
        if encoded.len() > MAX_POSTCONDITION_VERIFICATION_RECEIPT_BYTES
            || !is_sha256_hex(digest)
            || fingerprint(encoded) != *digest
        {
            return None;
        }
        let receipt = serde_json::from_str::<Self>(encoded).ok()?;
        receipt.contract_is_valid().then_some(receipt)
    }

    fn expected_digest(&self) -> Option<String> {
        serde_json::to_string(&(
            &self.schema,
            self.steer_epoch,
            self.contract_epoch,
            self.kind,
            &self.postcondition_id,
            self.action_sequence,
            self.observation_sequence,
            &self.verifier_source,
            &self.verifier_kind,
            &self.verifier_digest,
        ))
        .ok()
        .map(|encoded| fingerprint(&encoded))
    }
}

fn verifier_kind_matches_postcondition(value: &str, kind: OutcomePostconditionKind) -> bool {
    match kind {
        OutcomePostconditionKind::WorkspaceMutation => matches!(
            value,
            "workspace_exact_readback_v1" | "workspace_quality_check_v1"
        ),
        OutcomePostconditionKind::BrowserInteraction => value == BROWSER_OBSERVATION_VERIFIER,
        OutcomePostconditionKind::ComputerInteraction => value == COMPUTER_OBSERVATION_VERIFIER,
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
}
