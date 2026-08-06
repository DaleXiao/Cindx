use super::{
    is_sha256_hex, PostconditionActionBinding, PostconditionBindingKey, PostconditionTargetWitness,
    PostconditionVerificationReceipt, MAX_POSTCONDITION_TARGETS,
    MAX_POSTCONDITION_VERIFICATION_RECEIPTS, MAX_VERIFIER_CONTRACT_BYTES,
    MAX_VERIFIER_SOURCE_BYTES, WORKSPACE_FILE_CONTENT_VERIFIER,
};
use crate::task_contract::{
    fingerprint, interaction_action, interaction_observation_verifies, AgentTaskContract,
    ContractEvidence, ContractEvidenceKind, OutcomePostconditionKind,
};
use crate::InteractionSurface;
use agent_core::{PostconditionVerifierKind, ToolPostconditionEvidence, ToolSpec};
use std::collections::BTreeSet;

impl AgentTaskContract {
    pub fn verify_postcondition_receipt(
        &self,
        receipt: &PostconditionVerificationReceipt,
        steer_epoch: u64,
        contract_epoch: u64,
    ) -> bool {
        receipt.contract_is_valid()
            && receipt.steer_epoch == steer_epoch
            && receipt.contract_epoch == contract_epoch
            && self
                .postcondition_verification_receipts
                .iter()
                .any(|stored| stored == receipt)
            && self.receipt_matches_evidence(receipt)
    }

    pub(in crate::task_contract) fn postcondition_has_authoritative_receipt(
        &self,
        kind: OutcomePostconditionKind,
        action_sequence: u64,
        observation_sequence: u64,
    ) -> bool {
        self.postcondition_verification_receipts
            .iter()
            .any(|receipt| {
                receipt.kind == kind
                    && receipt.action_sequence == action_sequence
                    && receipt.observation_sequence == observation_sequence
                    && receipt.contract_is_valid()
                    && self.receipt_matches_evidence(receipt)
            })
    }

    pub(crate) fn validate_persisted_postcondition_state(
        &self,
        current_steer_epoch: u64,
        current_contract_epoch: u64,
    ) -> Result<(), &'static str> {
        if self.postcondition_bindings.len() > 3 {
            return Err("too many postcondition bindings");
        }
        for (key, binding) in &self.postcondition_bindings {
            if !binding_contract_is_valid(
                self,
                *key,
                binding,
                current_steer_epoch,
                current_contract_epoch,
            ) {
                return Err("postcondition binding state is invalid");
            }
        }
        if self.postcondition_verification_receipts.len() > MAX_POSTCONDITION_VERIFICATION_RECEIPTS
        {
            return Err("too many postcondition verification receipts");
        }
        let mut receipt_digests = BTreeSet::new();
        let mut receipt_lineage = BTreeSet::new();
        for receipt in &self.postcondition_verification_receipts {
            if receipt.steer_epoch > current_steer_epoch
                || receipt.contract_epoch > current_contract_epoch
                || !receipt_digests.insert(receipt.receipt_digest.clone())
                || !receipt_lineage.insert((
                    receipt.kind,
                    receipt.action_sequence,
                    receipt.observation_sequence,
                ))
                || !self.verify_postcondition_receipt(
                    receipt,
                    receipt.steer_epoch,
                    receipt.contract_epoch,
                )
            {
                return Err("postcondition verification receipt state is invalid");
            }
        }
        Ok(())
    }

    fn receipt_matches_evidence(&self, receipt: &PostconditionVerificationReceipt) -> bool {
        let Some(action) = self
            .evidence
            .iter()
            .find(|item| item.sequence == receipt.action_sequence)
        else {
            return false;
        };
        let Some(observation) = self
            .evidence
            .iter()
            .find(|item| item.sequence == receipt.observation_sequence)
        else {
            return false;
        };
        action_evidence_matches(receipt.kind, action.kind)
            && postcondition_id(receipt.kind, action.sequence, &action.source)
                == receipt.postcondition_id
            && observation.kind == observation_kind_for_postcondition(receipt.kind)
            && observation.source == receipt.verifier_source
            && action.sequence < observation.sequence
    }
}

pub(super) fn update_verifier_coverage(
    binding: &mut PostconditionActionBinding,
    tool_name: &str,
    tool_spec: Option<&ToolSpec>,
    tool_evidence: Option<&ToolPostconditionEvidence>,
    persisted_tool_evidence: Option<&crate::tool_runtime::ReplayedToolPostconditionEvidence>,
    lineage_scope: Option<&str>,
) -> bool {
    match binding.kind {
        OutcomePostconditionKind::WorkspaceMutation => {
            let (Some(spec), Some(action_targets)) = (tool_spec, binding.target_witness.as_ref())
            else {
                return false;
            };
            if binding.verifier_contract.as_deref() != Some(WORKSPACE_FILE_CONTENT_VERIFIER)
                || spec.name != tool_name
            {
                return false;
            }
            let newly_covered = match (tool_evidence, persisted_tool_evidence) {
                (Some(evidence), None) if spec.postcondition_verifiers.contains(&evidence.kind) => {
                    let Some(scope) =
                        lineage_scope.filter(|scope| action_targets.same_scope(scope))
                    else {
                        return false;
                    };
                    match evidence.kind {
                        PostconditionVerifierKind::WorkspaceExactReadbackV1 => {
                            let Some(verifier) = PostconditionTargetWitness::capture(
                                &evidence.target_input_json,
                                scope,
                            ) else {
                                return false;
                            };
                            action_targets.covered_by(&verifier)
                        }
                        PostconditionVerifierKind::WorkspaceQualityCheckV1
                            if quality_check_targets_workspace_root(
                                &evidence.target_input_json,
                            ) =>
                        {
                            action_targets.exact_digests.clone()
                        }
                        _ => Vec::new(),
                    }
                }
                (None, Some(evidence))
                    if spec.postcondition_verifiers.contains(&evidence.kind)
                        && action_targets.same_scope_digest(&evidence.scope_digest) =>
                {
                    match evidence.kind {
                        PostconditionVerifierKind::WorkspaceExactReadbackV1 => evidence
                            .target_witness
                            .as_ref()
                            .map(|verifier| action_targets.covered_by(verifier))
                            .unwrap_or_default(),
                        PostconditionVerifierKind::WorkspaceQualityCheckV1
                            if evidence.target_witness.is_none() =>
                        {
                            action_targets.exact_digests.clone()
                        }
                        _ => Vec::new(),
                    }
                }
                _ => Vec::new(),
            };
            if newly_covered.is_empty() {
                return false;
            }
            binding.covered_target_digests.extend(newly_covered);
            binding.covered_target_digests.sort();
            binding.covered_target_digests.dedup();
            action_targets.fully_covered_by(&binding.covered_target_digests)
        }
        OutcomePostconditionKind::BrowserInteraction
        | OutcomePostconditionKind::ComputerInteraction => {
            interaction_observation_verifies(&binding.action_source, tool_name)
        }
    }
}

pub(super) fn binding_lineage_is_current(
    contract: &AgentTaskContract,
    binding: &PostconditionActionBinding,
    steer_epoch: u64,
    contract_epoch: u64,
) -> bool {
    binding.contract_epoch == contract_epoch
        && binding.steer_epoch <= steer_epoch
        && contract.evidence.iter().any(|evidence| {
            evidence.sequence == binding.action_sequence
                && evidence.source == binding.action_source
                && action_evidence_matches(binding.kind, evidence.kind)
        })
}

fn binding_contract_is_valid(
    contract: &AgentTaskContract,
    key: PostconditionBindingKey,
    binding: &PostconditionActionBinding,
    current_steer_epoch: u64,
    current_contract_epoch: u64,
) -> bool {
    if binding.contract_epoch > binding.steer_epoch
        || binding.steer_epoch > current_steer_epoch
        || binding.contract_epoch > current_contract_epoch
        || binding.action_sequence == 0
        || binding.action_source.trim().is_empty()
        || binding.action_source.len() > MAX_VERIFIER_SOURCE_BYTES
        || postcondition_id(
            binding.kind,
            binding.action_sequence,
            &binding.action_source,
        ) != binding.postcondition_id
        || !binding_key_matches_kind(key, binding.kind)
        || !binding_lineage_is_current(
            contract,
            binding,
            current_steer_epoch,
            binding.contract_epoch,
        )
        || contract.evidence.iter().any(|evidence| {
            evidence.sequence > binding.action_sequence && evidence_matches_key(evidence, key)
        })
    {
        return false;
    }
    match binding.kind {
        OutcomePostconditionKind::WorkspaceMutation => {
            let Some(target) = binding.target_witness.as_ref() else {
                return false;
            };
            target.contract_is_valid()
                && binding.verifier_contract.as_deref() == Some(WORKSPACE_FILE_CONTENT_VERIFIER)
                && binding
                    .verifier_contract
                    .as_ref()
                    .is_some_and(|value| value.len() <= MAX_VERIFIER_CONTRACT_BYTES)
                && binding.covered_target_digests.len() <= MAX_POSTCONDITION_TARGETS
                && binding
                    .covered_target_digests
                    .windows(2)
                    .all(|pair| pair[0] < pair[1])
                && binding.covered_target_digests.iter().all(|digest| {
                    is_sha256_hex(digest) && target.exact_digests.binary_search(digest).is_ok()
                })
                && !target.fully_covered_by(&binding.covered_target_digests)
        }
        OutcomePostconditionKind::BrowserInteraction
        | OutcomePostconditionKind::ComputerInteraction => {
            binding.verifier_contract.is_none()
                && binding.target_witness.is_none()
                && binding.covered_target_digests.is_empty()
        }
    }
}

fn binding_key_matches_kind(key: PostconditionBindingKey, kind: OutcomePostconditionKind) -> bool {
    matches!(
        (key, kind),
        (
            PostconditionBindingKey::Workspace,
            OutcomePostconditionKind::WorkspaceMutation
        ) | (
            PostconditionBindingKey::Browser,
            OutcomePostconditionKind::BrowserInteraction
        ) | (
            PostconditionBindingKey::Computer,
            OutcomePostconditionKind::ComputerInteraction
        )
    )
}

fn evidence_matches_key(evidence: &ContractEvidence, key: PostconditionBindingKey) -> bool {
    match key {
        PostconditionBindingKey::Workspace => evidence.kind == ContractEvidenceKind::Mutation,
        PostconditionBindingKey::Browser => {
            evidence.kind == ContractEvidenceKind::InteractionAction
                && interaction_action(&evidence.source)
                    .is_some_and(|(surface, _)| surface == InteractionSurface::Browser)
        }
        PostconditionBindingKey::Computer => {
            evidence.kind == ContractEvidenceKind::InteractionAction
                && interaction_action(&evidence.source)
                    .is_some_and(|(surface, _)| surface == InteractionSurface::Computer)
        }
    }
}

pub(super) fn action_evidence_matches(
    kind: OutcomePostconditionKind,
    evidence: ContractEvidenceKind,
) -> bool {
    matches!(
        (kind, evidence),
        (
            OutcomePostconditionKind::WorkspaceMutation,
            ContractEvidenceKind::Mutation
        ) | (
            OutcomePostconditionKind::BrowserInteraction
                | OutcomePostconditionKind::ComputerInteraction,
            ContractEvidenceKind::InteractionAction
        )
    )
}

pub(super) fn observation_kind_for_postcondition(
    kind: OutcomePostconditionKind,
) -> ContractEvidenceKind {
    match kind {
        OutcomePostconditionKind::WorkspaceMutation => ContractEvidenceKind::Verification,
        OutcomePostconditionKind::BrowserInteraction
        | OutcomePostconditionKind::ComputerInteraction => {
            ContractEvidenceKind::InteractionObservation
        }
    }
}

fn quality_check_targets_workspace_root(input_json: &str) -> bool {
    crate::task_contract::structured_targets(input_json) == BTreeSet::from([".".to_string()])
}

pub(super) fn postcondition_id(
    kind: OutcomePostconditionKind,
    action_sequence: u64,
    action_source: &str,
) -> String {
    let namespace = match kind {
        OutcomePostconditionKind::WorkspaceMutation => "workspace_mutation",
        OutcomePostconditionKind::BrowserInteraction
        | OutcomePostconditionKind::ComputerInteraction => "interaction",
    };
    fingerprint(&format!("{namespace}|{action_sequence}|{action_source}"))
}
