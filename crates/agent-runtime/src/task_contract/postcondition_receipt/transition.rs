use super::validation::{
    action_evidence_matches, binding_lineage_is_current, observation_kind_for_postcondition,
    postcondition_id, update_verifier_coverage,
};
use super::{
    PostconditionActionBinding, PostconditionBindingKey, PostconditionTargetWitness,
    PostconditionVerificationReceipt, MAX_POSTCONDITION_VERIFICATION_RECEIPTS,
    MAX_VERIFIER_CONTRACT_BYTES, WORKSPACE_FILE_CONTENT_VERIFIER,
};
use crate::task_contract::{
    fingerprint, inferred_builtin_tool_risk, interaction_action, interaction_observation,
    substantive_observation, AgentTaskContract, OutcomePostconditionKind,
};
use crate::InteractionSurface;
use agent_core::{ToolOutcomeStatus, ToolPostconditionEvidence, ToolRisk, ToolSpec};

impl AgentTaskContract {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_tool_outcome_transition(
        &mut self,
        steer_epoch: u64,
        contract_epoch: u64,
        tool_name: &str,
        input_json: &str,
        status: &ToolOutcomeStatus,
        risk: Option<&ToolRisk>,
        tool_spec: Option<&ToolSpec>,
        tool_evidence: Option<&ToolPostconditionEvidence>,
        persisted_tool_evidence: Option<&crate::tool_runtime::ReplayedToolPostconditionEvidence>,
        observation: &str,
        target_witness: Option<&PostconditionTargetWitness>,
        lineage_scope: Option<&str>,
        allow_action_binding: bool,
    ) -> Option<PostconditionVerificationReceipt> {
        let effective_tool_name = tool_spec
            .map(|spec| spec.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(tool_name);
        let evidence_watermark = self.next_sequence;
        let observation_key = postcondition_observation_key(effective_tool_name, risk);
        let binding =
            observation_key.and_then(|key| self.postcondition_bindings.get(&key).cloned());
        let observation_is_usable = matches!(status, ToolOutcomeStatus::Succeeded)
            && contract_epoch <= steer_epoch
            && (substantive_observation(observation)
                || tool_evidence.is_some()
                || persisted_tool_evidence.is_some());
        let mut updated_binding = binding.clone();
        let verification_authorized = observation_is_usable
            && updated_binding.as_mut().is_some_and(|binding| {
                binding_lineage_is_current(self, binding, steer_epoch, contract_epoch)
                    && update_verifier_coverage(
                        binding,
                        effective_tool_name,
                        tool_spec,
                        tool_evidence,
                        persisted_tool_evidence,
                        lineage_scope,
                    )
            });
        let rollback = verification_authorized.then(|| self.clone());
        if let (Some(key), Some(binding)) = (observation_key, updated_binding) {
            self.postcondition_bindings.insert(key, binding);
        }

        // Once the typed transition path is active, every recognized observation
        // is explicit. A recovered legacy action without a binding must therefore
        // fail closed instead of falling back to text/target heuristics.
        let explicit_verification = observation_key.map(|_| verification_authorized);
        self.record_tool_outcome_internal(
            effective_tool_name,
            input_json,
            status,
            risk,
            explicit_verification,
        );

        if !matches!(status, ToolOutcomeStatus::Succeeded) {
            return None;
        }
        let new_evidence = self
            .evidence
            .last()
            .filter(|evidence| evidence.sequence > evidence_watermark)
            .cloned()?;
        if let Some((key, kind)) = postcondition_action_key(effective_tool_name, risk) {
            // A successful action always supersedes the prior binding, even if
            // this action lacks a typed verifier contract and must fail closed.
            self.postcondition_bindings.remove(&key);
            if allow_action_binding
                && contract_epoch <= steer_epoch
                && action_evidence_matches(kind, new_evidence.kind)
            {
                let target_witness = if kind == OutcomePostconditionKind::WorkspaceMutation {
                    target_witness.cloned().or_else(|| {
                        lineage_scope.and_then(|scope| {
                            PostconditionTargetWitness::capture(input_json, scope)
                        })
                    })
                } else {
                    None
                };
                let verifier_contract = (kind == OutcomePostconditionKind::WorkspaceMutation)
                    .then(|| workspace_action_verifier_contract(tool_spec))
                    .flatten();
                let may_bind = kind != OutcomePostconditionKind::WorkspaceMutation
                    || (target_witness.is_some() && verifier_contract.is_some());
                if may_bind {
                    self.postcondition_bindings.insert(
                        key,
                        PostconditionActionBinding {
                            steer_epoch,
                            contract_epoch,
                            kind,
                            postcondition_id: postcondition_id(
                                kind,
                                new_evidence.sequence,
                                &new_evidence.source,
                            ),
                            action_sequence: new_evidence.sequence,
                            action_source: new_evidence.source,
                            verifier_contract,
                            target_witness,
                            covered_target_digests: Vec::new(),
                        },
                    );
                }
            }
            return None;
        }
        if !verification_authorized {
            return None;
        }
        let binding = binding?;
        let verifier_kind =
            verifier_kind_for_transition(binding.kind, tool_evidence, persisted_tool_evidence)?;
        let expected_observation_kind = observation_kind_for_postcondition(binding.kind);
        let receipt = (new_evidence.kind == expected_observation_kind
            && new_evidence.source == effective_tool_name)
            .then(|| {
                PostconditionVerificationReceipt::new(
                    steer_epoch,
                    contract_epoch,
                    binding.kind,
                    binding.postcondition_id,
                    binding.action_sequence,
                    new_evidence.sequence,
                    effective_tool_name,
                    verifier_kind,
                    verifier_digest(effective_tool_name, verifier_kind, input_json, observation),
                )
            })
            .flatten();
        let Some(receipt) = receipt else {
            return self.rollback_invalid_verification(
                rollback,
                effective_tool_name,
                input_json,
                status,
                risk,
            );
        };
        self.postcondition_verification_receipts
            .push(receipt.clone());
        let dropped = self
            .postcondition_verification_receipts
            .len()
            .saturating_sub(MAX_POSTCONDITION_VERIFICATION_RECEIPTS);
        if dropped > 0 {
            self.postcondition_verification_receipts.drain(..dropped);
        }
        if !self.verify_postcondition_receipt(&receipt, steer_epoch, contract_epoch) {
            return self.rollback_invalid_verification(
                rollback,
                effective_tool_name,
                input_json,
                status,
                risk,
            );
        }
        if let Some(key) = observation_key {
            self.postcondition_bindings.remove(&key);
        }
        Some(receipt)
    }

    fn rollback_invalid_verification(
        &mut self,
        rollback: Option<Self>,
        tool_name: &str,
        input_json: &str,
        status: &ToolOutcomeStatus,
        risk: Option<&ToolRisk>,
    ) -> Option<PostconditionVerificationReceipt> {
        if let Some(previous) = rollback {
            *self = previous;
            self.record_tool_outcome_internal(tool_name, input_json, status, risk, Some(false));
        }
        None
    }
}

fn postcondition_action_key(
    tool_name: &str,
    risk: Option<&ToolRisk>,
) -> Option<(PostconditionBindingKey, OutcomePostconditionKind)> {
    if let Some((surface, _)) = interaction_action(tool_name) {
        return Some(match surface {
            InteractionSurface::Browser => (
                PostconditionBindingKey::Browser,
                OutcomePostconditionKind::BrowserInteraction,
            ),
            InteractionSurface::Computer => (
                PostconditionBindingKey::Computer,
                OutcomePostconditionKind::ComputerInteraction,
            ),
        });
    }
    matches!(
        risk.or_else(|| inferred_builtin_tool_risk(tool_name)),
        Some(ToolRisk::WritesWorkspace | ToolRisk::Destructive)
    )
    .then_some((
        PostconditionBindingKey::Workspace,
        OutcomePostconditionKind::WorkspaceMutation,
    ))
}

fn postcondition_observation_key(
    tool_name: &str,
    risk: Option<&ToolRisk>,
) -> Option<PostconditionBindingKey> {
    if let Some(surface) = interaction_observation(tool_name) {
        return Some(match surface {
            InteractionSurface::Browser => PostconditionBindingKey::Browser,
            InteractionSurface::Computer => PostconditionBindingKey::Computer,
        });
    }
    matches!(
        risk.or_else(|| inferred_builtin_tool_risk(tool_name)),
        Some(ToolRisk::ReadOnly | ToolRisk::ExecutesProcess)
    )
    .then_some(PostconditionBindingKey::Workspace)
}

fn verifier_kind_for_transition(
    kind: OutcomePostconditionKind,
    evidence: Option<&ToolPostconditionEvidence>,
    persisted_evidence: Option<&crate::tool_runtime::ReplayedToolPostconditionEvidence>,
) -> Option<&'static str> {
    match kind {
        OutcomePostconditionKind::WorkspaceMutation => evidence
            .map(|item| item.kind.label())
            .or_else(|| persisted_evidence.map(|item| item.kind.label())),
        OutcomePostconditionKind::BrowserInteraction => Some(super::BROWSER_OBSERVATION_VERIFIER),
        OutcomePostconditionKind::ComputerInteraction => Some(super::COMPUTER_OBSERVATION_VERIFIER),
    }
}

fn workspace_action_verifier_contract(tool_spec: Option<&ToolSpec>) -> Option<String> {
    tool_spec
        .and_then(|spec| spec.effect_semantics.verifier())
        .filter(|value| *value == WORKSPACE_FILE_CONTENT_VERIFIER)
        .filter(|value| !value.trim().is_empty() && value.len() <= MAX_VERIFIER_CONTRACT_BYTES)
        .map(str::to_string)
}

fn verifier_digest(source: &str, kind: &str, input: &str, observation: &str) -> String {
    fingerprint(
        &serde_json::to_string(&(source, kind, fingerprint(input), fingerprint(observation)))
            .unwrap_or_default(),
    )
}
