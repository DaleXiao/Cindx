use crate::evidence_target::{
    evidence_input_matches_anchors, evidence_target_witness_matches, EvidenceTargetAnchor,
};
use crate::{AgentFailure, InteractionSurface};
use agent_core::{ToolOutcomeStatus, ToolRisk, ToolSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

mod grounded_completion;
mod outcome_ledger;

pub use grounded_completion::{
    GroundedCompletionBasis, GroundedCompletionIssue, GroundedCompletionReceipt,
    GROUNDED_COMPLETION_DIGEST_METADATA_KEY, GROUNDED_COMPLETION_METADATA_KEY,
    GROUNDED_COMPLETION_SCHEMA,
};

pub use outcome_ledger::{
    OutcomeClaim, OutcomeClaimDecision, OutcomeClaimEvidenceStatus, OutcomeClaimKind,
    OutcomeClaimQuality, OutcomeEvidence, OutcomeFailure, OutcomeFailureClass, OutcomeLedgerPhase,
    OutcomeLedgerShadow, OutcomeObligation, OutcomeObligationKind, OutcomePostcondition,
    OutcomePostconditionKind, OutcomePostconditionStatus, OutcomeSatisfaction, OutcomeScope,
    OutcomeTerminal, OutcomeTerminalObservation, OutcomeTruncation,
    OUTCOME_LEDGER_DIGEST_METADATA_KEY, OUTCOME_LEDGER_MAX_METADATA_BYTES,
    OUTCOME_LEDGER_METADATA_KEY, OUTCOME_LEDGER_SCHEMA,
};

const MAX_CONTRACT_EVIDENCE: usize = 128;
const MAX_COMPLETION_GATE_ATTEMPTS: usize = 2;
const MAX_CONTEXT_TARGETS: usize = 8;
const MAX_CONTEXT_EVIDENCE: usize = 8;
const MAX_GROUNDING_EXCERPT_CHARS: usize = 1_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractEvidenceKind {
    RequiredTool,
    Grounding,
    Read,
    Mutation,
    Verification,
    InteractionAction,
    InteractionObservation,
    OtherTool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceVerificationPolicy {
    #[default]
    NotRequired,
    RequiredAfterMutation,
}

impl WorkspaceVerificationPolicy {
    pub fn is_required(self) -> bool {
        self == Self::RequiredAfterMutation
    }

    fn merge(self, other: Self) -> Self {
        if self.is_required() || other.is_required() {
            Self::RequiredAfterMutation
        } else {
            Self::NotRequired
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractEvidence {
    pub sequence: u64,
    pub kind: ContractEvidenceKind,
    pub source: String,
    pub input_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptEvidenceContext {
    pub requirement_id: String,
    pub source: String,
    pub observation: String,
    pub evidence_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptEvidenceReceipt {
    source: String,
    observation: String,
    context_backed: bool,
    evidence_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PromptEvidenceRequirement {
    #[serde(default)]
    tools: BTreeSet<String>,
    #[serde(skip, default)]
    target_anchors: BTreeSet<EvidenceTargetAnchor>,
    // Observations are request-scoped model context, not durable state. A cold
    // restore therefore fails closed and re-observes instead of persisting
    // potentially sensitive tool output or claiming evidence the model cannot see.
    #[serde(skip, default)]
    receipt: Option<PromptEvidenceReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractContext {
    schema: &'static str,
    unresolved_required_tools: Vec<String>,
    unresolved_any_tool_requirements: Vec<TaskContractAnyToolRequirement>,
    pending_postconditions: Vec<TaskContractPostcondition>,
    unverified_workspace_mutation: Option<TaskContractMutation>,
    recent_evidence: Vec<TaskContractEvidenceSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractAnyToolRequirement {
    id: String,
    alternatives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractPostcondition {
    surface: &'static str,
    action: String,
    observers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractMutation {
    epoch: u64,
    targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractEvidenceSummary {
    sequence: u64,
    kind: ContractEvidenceKind,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskContract {
    #[serde(default)]
    workspace_verification_policy: WorkspaceVerificationPolicy,
    #[serde(default)]
    required_tool_successes: BTreeSet<String>,
    #[serde(default)]
    required_any_tool_successes: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    successful_tools: BTreeSet<String>,
    #[serde(default)]
    prompt_requirement_epoch: u64,
    #[serde(default)]
    prompt_required_tool_successes: BTreeSet<String>,
    #[serde(default)]
    prompt_required_any_tool_successes: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    prompt_successful_tools: BTreeSet<String>,
    #[serde(default)]
    prompt_evidence_epoch: u64,
    #[serde(default)]
    prompt_evidence_requirements: BTreeMap<String, PromptEvidenceRequirement>,
    #[serde(default)]
    mutation_targets: BTreeSet<String>,
    #[serde(default)]
    mutation_epoch: u64,
    #[serde(default)]
    verified_mutation_epoch: u64,
    #[serde(default)]
    pending_interactions: BTreeMap<InteractionSurface, String>,
    #[serde(default)]
    evidence: Vec<ContractEvidence>,
    #[serde(default)]
    next_sequence: u64,
    #[serde(default)]
    gate_attempts: BTreeMap<String, usize>,
    #[serde(default)]
    outcome_claims: Vec<OutcomeClaim>,
    #[serde(default)]
    outcome_dropped_claims: u64,
    #[serde(default)]
    next_outcome_claim_sequence: u64,
}

impl AgentTaskContract {
    pub fn merge_workspace_verification_policy(&mut self, policy: WorkspaceVerificationPolicy) {
        self.workspace_verification_policy = self.workspace_verification_policy.merge(policy);
    }

    pub fn workspace_verification_policy(&self) -> WorkspaceVerificationPolicy {
        self.workspace_verification_policy
    }

    pub fn require_tool_success(&mut self, tool_name: impl Into<String>) {
        let tool_name = tool_name.into();
        if !tool_name.trim().is_empty() {
            self.required_tool_successes.insert(tool_name);
        }
    }

    /// Replaces requirements derived from the active prompt.
    ///
    /// Prompt-scoped success evidence is retained when the same epoch is
    /// replayed, but is discarded when steering advances to a new epoch.
    /// Run-wide requirements and evidence are deliberately unaffected.
    pub fn replace_prompt_required_tool_successes<I, S>(&mut self, epoch: u64, tools: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let required_tools = tools
            .into_iter()
            .map(Into::into)
            .map(|tool: String| tool.trim().to_string())
            .filter(|tool| !tool.is_empty())
            .collect::<BTreeSet<_>>();
        let epoch_changed = self.prompt_requirement_epoch != epoch;
        let requirements_changed = self.prompt_required_tool_successes != required_tools;

        if epoch_changed {
            self.prompt_requirement_epoch = epoch;
            self.prompt_successful_tools.clear();
        }
        if epoch_changed || requirements_changed {
            self.gate_attempts
                .retain(|key, _| !key.starts_with("prompt_tool:"));
        }
        self.prompt_required_tool_successes = required_tools;
    }

    /// Replaces capability requirements derived from the active prompt and
    /// conductor decision. A success from an older steer epoch cannot satisfy
    /// the current objective.
    pub fn replace_prompt_required_any_tool_successes(
        &mut self,
        epoch: u64,
        requirements: BTreeMap<String, BTreeSet<String>>,
    ) {
        let requirements = requirements
            .into_iter()
            .filter_map(|(requirement_id, tools)| {
                let requirement_id = requirement_id.trim().to_string();
                let tools = tools
                    .into_iter()
                    .map(|tool| tool.trim().to_string())
                    .filter(|tool| !tool.is_empty())
                    .collect::<BTreeSet<_>>();
                (!requirement_id.is_empty() && !tools.is_empty()).then_some((requirement_id, tools))
            })
            .collect::<BTreeMap<_, _>>();
        let epoch_changed = self.prompt_requirement_epoch != epoch;
        let requirements_changed = self.prompt_required_any_tool_successes != requirements;

        if epoch_changed {
            self.prompt_requirement_epoch = epoch;
            self.prompt_successful_tools.clear();
        }
        if epoch_changed || requirements_changed {
            self.gate_attempts
                .retain(|key, _| !key.starts_with("prompt_any_tool:"));
        }
        self.prompt_required_any_tool_successes = requirements;
    }

    /// Replaces the substantive evidence obligation derived from the active prompt.
    ///
    /// Evidence is scoped to the current steer epoch. Replaying the same persisted
    /// contract retains committed evidence, while a new objective or requirement
    /// clears it so an earlier task cannot satisfy the new one.
    pub fn replace_prompt_evidence_requirements(
        &mut self,
        epoch: u64,
        requirements: BTreeMap<String, BTreeSet<String>>,
    ) {
        let requirements = requirements
            .into_iter()
            .filter_map(|(requirement_id, tools)| {
                let requirement_id = requirement_id.trim().to_string();
                if requirement_id.is_empty() {
                    return None;
                }
                let tools = tools
                    .into_iter()
                    .map(|tool| tool.trim().to_string())
                    .filter(|tool| !tool.is_empty())
                    .collect::<BTreeSet<_>>();
                Some((requirement_id, tools))
            })
            .collect::<BTreeMap<_, _>>();
        let current_tools = self
            .prompt_evidence_requirements
            .iter()
            .map(|(id, requirement)| (id.clone(), requirement.tools.clone()))
            .collect::<BTreeMap<_, _>>();
        let changed = self.prompt_evidence_epoch != epoch || current_tools != requirements;

        if changed {
            self.prompt_evidence_requirements = requirements
                .into_iter()
                .map(|(id, tools)| {
                    (
                        id,
                        PromptEvidenceRequirement {
                            tools,
                            target_anchors: BTreeSet::new(),
                            receipt: None,
                        },
                    )
                })
                .collect();
            self.gate_attempts
                .retain(|key, _| !key.starts_with("prompt_evidence:"));
        }
        self.prompt_evidence_epoch = epoch;
    }

    /// Binds conservative, runtime-only targets to the current prompt evidence
    /// requirements. Raw targets are deliberately excluded from snapshots.
    pub fn bind_prompt_evidence_targets(
        &mut self,
        epoch: u64,
        targets: BTreeMap<String, BTreeSet<EvidenceTargetAnchor>>,
    ) {
        if self.prompt_evidence_epoch != epoch {
            return;
        }
        for (requirement_id, requirement) in &mut self.prompt_evidence_requirements {
            requirement.target_anchors = targets.get(requirement_id).cloned().unwrap_or_default();
        }
    }

    pub fn replace_prompt_evidence_requirement<I, S>(
        &mut self,
        epoch: u64,
        requirement_id: Option<&str>,
        tools: I,
    ) where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let requirements = requirement_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|requirement_id| {
                [(
                    requirement_id.to_string(),
                    tools
                        .into_iter()
                        .map(Into::into)
                        .collect::<BTreeSet<String>>(),
                )]
                .into_iter()
                .collect()
            })
            .unwrap_or_default();
        self.replace_prompt_evidence_requirements(epoch, requirements);
    }

    /// Records a trusted, substantive observation for one prompt obligation.
    /// The bounded observation remains runtime-only so persisted recovery
    /// cannot leak tool output or silently complete without visible evidence.
    pub fn record_prompt_evidence_for_requirement_at(
        &mut self,
        epoch: u64,
        requirement_id: &str,
        source: &str,
        receipt: &str,
        observation: &str,
    ) -> bool {
        self.record_prompt_evidence_at(
            epoch,
            requirement_id,
            source,
            receipt,
            observation,
            false,
            false,
        )
    }

    pub fn record_prompt_context_evidence_for_requirement_at(
        &mut self,
        epoch: u64,
        requirement_id: &str,
        source: &str,
        receipt: &str,
        observation: &str,
    ) -> bool {
        self.record_prompt_evidence_at(
            epoch,
            requirement_id,
            source,
            receipt,
            observation,
            true,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_prompt_evidence_at(
        &mut self,
        epoch: u64,
        requirement_id: &str,
        source: &str,
        receipt: &str,
        observation: &str,
        context_backed: bool,
        target_witness_trusted: bool,
    ) -> bool {
        if self.prompt_evidence_epoch != epoch
            || source.trim().is_empty()
            || !substantive_observation(observation)
        {
            return false;
        }
        let Some(requirement) = self.prompt_evidence_requirements.get(requirement_id) else {
            return false;
        };
        if requirement.tools.contains(source)
            && !evidence_input_matches_anchors(receipt, &requirement.target_anchors)
            && !(target_witness_trusted
                && evidence_target_witness_matches(
                    receipt,
                    &requirement.target_anchors,
                    source,
                    epoch,
                ))
        {
            return false;
        }
        if requirement.receipt.is_some() {
            return true;
        }
        self.record_evidence(ContractEvidenceKind::Grounding, source, receipt);
        let evidence_sequence = self.next_sequence;
        let Some(requirement) = self.prompt_evidence_requirements.get_mut(requirement_id) else {
            return false;
        };
        requirement.receipt = Some(PromptEvidenceReceipt {
            source: source.to_string(),
            observation: bounded_grounding_excerpt(observation),
            context_backed,
            evidence_sequence,
        });
        let gate_key = format!(
            "prompt_evidence:{}:{requirement_id}",
            self.prompt_evidence_epoch
        );
        self.gate_attempts.remove(&gate_key);
        true
    }

    pub fn record_prompt_tool_evidence_observation_at(
        &mut self,
        epoch: u64,
        tool_name: &str,
        source: &str,
        receipt: &str,
        observation: &str,
    ) -> bool {
        self.record_prompt_tool_evidence_observation_with_trust_at(
            epoch,
            tool_name,
            source,
            receipt,
            observation,
            false,
        )
    }

    pub fn record_persisted_prompt_tool_evidence_observation_at(
        &mut self,
        epoch: u64,
        tool_name: &str,
        source: &str,
        receipt: &str,
        observation: &str,
    ) -> bool {
        self.record_prompt_tool_evidence_observation_with_trust_at(
            epoch,
            tool_name,
            source,
            receipt,
            observation,
            true,
        )
    }

    fn record_prompt_tool_evidence_observation_with_trust_at(
        &mut self,
        epoch: u64,
        tool_name: &str,
        source: &str,
        receipt: &str,
        observation: &str,
        target_witness_trusted: bool,
    ) -> bool {
        if self.prompt_evidence_epoch != epoch || !substantive_observation(observation) {
            return false;
        }
        let requirement_ids = self
            .prompt_evidence_requirements
            .iter()
            .filter(|(_, requirement)| requirement.tools.contains(tool_name))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut recorded = false;
        for requirement_id in requirement_ids {
            recorded |= self.record_prompt_evidence_at(
                epoch,
                &requirement_id,
                source,
                receipt,
                observation,
                false,
                target_witness_trusted,
            );
        }
        recorded
    }

    pub fn prompt_evidence_contexts(&self) -> Vec<PromptEvidenceContext> {
        self.prompt_evidence_requirements
            .iter()
            .filter_map(|(requirement_id, requirement)| {
                requirement.receipt.as_ref().and_then(|receipt| {
                    (!receipt.context_backed).then(|| PromptEvidenceContext {
                        requirement_id: requirement_id.clone(),
                        source: receipt.source.clone(),
                        observation: receipt.observation.clone(),
                        evidence_sequence: receipt.evidence_sequence,
                    })
                })
            })
            .collect()
    }

    pub fn has_prompt_evidence(&self) -> bool {
        self.prompt_evidence_requirements
            .values()
            .any(|requirement| requirement.receipt.is_some())
    }

    pub fn prompt_evidence_sequence(&self, epoch: u64, requirement_id: &str) -> Option<u64> {
        (self.prompt_evidence_epoch == epoch)
            .then(|| {
                self.prompt_evidence_requirements
                    .get(requirement_id)?
                    .receipt
                    .as_ref()
                    .map(|receipt| receipt.evidence_sequence)
            })
            .flatten()
    }

    pub fn prompt_evidence_epoch(&self) -> u64 {
        self.prompt_evidence_epoch
    }

    pub fn require_any_tool_success<I, S>(&mut self, requirement_id: impl Into<String>, tools: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let requirement_id = requirement_id.into();
        let requirement_id = requirement_id.trim();
        if requirement_id.is_empty() {
            return;
        }
        let alternatives = tools
            .into_iter()
            .map(Into::into)
            .map(|tool: String| tool.trim().to_string())
            .filter(|tool| !tool.is_empty())
            .collect::<BTreeSet<_>>();
        if alternatives.is_empty() {
            return;
        }
        self.required_any_tool_successes
            .entry(requirement_id.to_string())
            .or_default()
            .extend(alternatives);
    }

    pub fn required_tool_satisfied(&self, tool_name: &str) -> bool {
        if self.prompt_required_tool_successes.contains(tool_name) {
            self.prompt_successful_tools.contains(tool_name)
        } else {
            self.successful_tools.contains(tool_name)
        }
    }

    pub fn successful_mutations(&self) -> usize {
        self.mutation_epoch as usize
    }

    pub fn latest_mutation_verified(&self) -> bool {
        self.mutation_epoch == 0 || self.verified_mutation_epoch >= self.mutation_epoch
    }

    pub fn pending_interactions(&self) -> &BTreeMap<InteractionSurface, String> {
        &self.pending_interactions
    }

    pub fn evidence(&self) -> &[ContractEvidence] {
        &self.evidence
    }

    /// Returns the active task obligations as trusted, bounded runtime data.
    ///
    /// Rendering this context is deliberately side-effect free: repair-attempt
    /// accounting belongs to the completion gate, not to ordinary model turns.
    pub fn model_context(&self, verification_required: bool, tools: &[ToolSpec]) -> Option<String> {
        self.model_context_with_requirement(verification_required, tools)
    }

    pub fn model_context_for_task(&self, tools: &[ToolSpec]) -> Option<String> {
        self.model_context_with_requirement(self.workspace_verification_policy.is_required(), tools)
    }

    fn model_context_with_requirement(
        &self,
        verification_required: bool,
        tools: &[ToolSpec],
    ) -> Option<String> {
        let available_tools = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<BTreeSet<_>>();
        let unresolved_required_tools = self
            .required_tool_successes
            .iter()
            .filter(|tool_name| !self.successful_tools.contains(*tool_name))
            .cloned()
            .chain(
                self.prompt_required_tool_successes
                    .iter()
                    .filter(|tool_name| !self.prompt_successful_tools.contains(*tool_name))
                    .cloned(),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut unresolved_any_tool_requirements = self
            .required_any_tool_successes
            .iter()
            .filter(|(_, alternatives)| alternatives.is_disjoint(&self.successful_tools))
            .map(|(id, alternatives)| TaskContractAnyToolRequirement {
                id: id.clone(),
                alternatives: alternatives
                    .iter()
                    .filter(|tool| available_tools.contains(tool.as_str()))
                    .cloned()
                    .collect(),
            })
            .collect::<Vec<_>>();
        unresolved_any_tool_requirements.extend(
            self.prompt_required_any_tool_successes
                .iter()
                .filter(|(_, alternatives)| alternatives.is_disjoint(&self.prompt_successful_tools))
                .map(|(id, alternatives)| TaskContractAnyToolRequirement {
                    id: id.clone(),
                    alternatives: alternatives
                        .iter()
                        .filter(|tool| available_tools.contains(tool.as_str()))
                        .cloned()
                        .collect(),
                }),
        );
        unresolved_any_tool_requirements.extend(
            self.prompt_evidence_requirements
                .iter()
                .filter(|(_, requirement)| requirement.receipt.is_none())
                .map(|(id, requirement)| TaskContractAnyToolRequirement {
                    id: id.clone(),
                    alternatives: requirement
                        .tools
                        .iter()
                        .filter(|tool| available_tools.contains(tool.as_str()))
                        .cloned()
                        .collect(),
                }),
        );
        let pending_postconditions = self
            .pending_interactions
            .iter()
            .filter_map(|(surface, action)| {
                let observers = interaction_observers(*surface, action)
                    .iter()
                    .copied()
                    .filter(|candidate| available_tools.contains(candidate))
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                (!observers.is_empty()).then(|| TaskContractPostcondition {
                    surface: surface.label(),
                    action: action.clone(),
                    observers,
                })
            })
            .collect::<Vec<_>>();
        let unverified_workspace_mutation = (verification_required
            && self.mutation_epoch > 0
            && !self.latest_mutation_verified()
            && tools.iter().any(tool_can_verify_workspace_change))
        .then(|| TaskContractMutation {
            epoch: self.mutation_epoch,
            targets: self
                .mutation_targets
                .iter()
                .take(MAX_CONTEXT_TARGETS)
                .cloned()
                .collect(),
        });

        if unresolved_required_tools.is_empty()
            && unresolved_any_tool_requirements.is_empty()
            && pending_postconditions.is_empty()
            && unverified_workspace_mutation.is_none()
        {
            return None;
        }

        let recent_evidence = self
            .evidence
            .iter()
            .rev()
            .take(MAX_CONTEXT_EVIDENCE)
            .rev()
            .map(|evidence| TaskContractEvidenceSummary {
                sequence: evidence.sequence,
                kind: evidence.kind,
                source: evidence.source.clone(),
            })
            .collect();
        let context = TaskContractContext {
            schema: "cindx.task-contract.v1",
            unresolved_required_tools,
            unresolved_any_tool_requirements,
            pending_postconditions,
            unverified_workspace_mutation,
            recent_evidence,
        };
        serde_json::to_string(&context).ok()
    }

    pub(crate) fn restore_legacy(
        successful_mutations: usize,
        verified_after_last_mutation: bool,
        pending_interactions: BTreeMap<InteractionSurface, String>,
    ) -> Self {
        Self {
            mutation_epoch: successful_mutations as u64,
            verified_mutation_epoch: if verified_after_last_mutation {
                successful_mutations as u64
            } else {
                0
            },
            pending_interactions,
            ..Self::default()
        }
    }

    pub fn record_tool_outcome(
        &mut self,
        tool_name: &str,
        input_json: &str,
        status: &ToolOutcomeStatus,
        risk: Option<&ToolRisk>,
    ) {
        if !matches!(status, ToolOutcomeStatus::Succeeded) {
            return;
        }

        self.successful_tools.insert(tool_name.to_string());
        self.prompt_successful_tools.insert(tool_name.to_string());
        for requirement_id in self
            .required_any_tool_successes
            .iter()
            .filter(|(_, alternatives)| alternatives.contains(tool_name))
            .map(|(requirement_id, _)| requirement_id.clone())
            .collect::<Vec<_>>()
        {
            self.gate_attempts
                .remove(&format!("any_tool:{requirement_id}"));
        }
        for requirement_id in self
            .prompt_required_any_tool_successes
            .iter()
            .filter(|(_, alternatives)| alternatives.contains(tool_name))
            .map(|(requirement_id, _)| requirement_id.clone())
            .collect::<Vec<_>>()
        {
            self.gate_attempts.remove(&format!(
                "prompt_any_tool:{}:{requirement_id}",
                self.prompt_requirement_epoch
            ));
        }
        if self.required_tool_successes.contains(tool_name) {
            self.gate_attempts.remove(&format!("tool:{tool_name}"));
        }
        if self.prompt_required_tool_successes.contains(tool_name) {
            self.gate_attempts.remove(&format!(
                "prompt_tool:{}:{tool_name}",
                self.prompt_requirement_epoch
            ));
        }
        if self.required_tool_successes.contains(tool_name)
            || self.prompt_required_tool_successes.contains(tool_name)
        {
            self.record_evidence(ContractEvidenceKind::RequiredTool, tool_name, input_json);
        }

        if let Some((surface, action)) = interaction_action(tool_name) {
            self.pending_interactions
                .insert(surface, action.to_string());
            self.gate_attempts.remove("interaction_verification");
            self.record_evidence(
                ContractEvidenceKind::InteractionAction,
                tool_name,
                input_json,
            );
            return;
        }
        if let Some(surface) = interaction_observation(tool_name) {
            let verified = self
                .pending_interactions
                .get(&surface)
                .is_some_and(|action| interaction_observation_verifies(action, tool_name));
            if verified {
                self.pending_interactions.remove(&surface);
                self.gate_attempts.remove("interaction_verification");
            }
            self.record_evidence(
                ContractEvidenceKind::InteractionObservation,
                tool_name,
                input_json,
            );
            return;
        }

        match risk.or_else(|| inferred_builtin_tool_risk(tool_name)) {
            Some(ToolRisk::WritesWorkspace | ToolRisk::Destructive) => {
                self.mutation_epoch = self.mutation_epoch.saturating_add(1);
                self.mutation_targets = structured_targets(input_json);
                self.gate_attempts.remove("workspace_verification");
                self.record_evidence(ContractEvidenceKind::Mutation, tool_name, input_json);
            }
            Some(ToolRisk::ReadOnly) => {
                let verifies_mutation = self.mutation_epoch > self.verified_mutation_epoch
                    && verification_targets_match(&self.mutation_targets, input_json);
                if verifies_mutation {
                    self.verified_mutation_epoch = self.mutation_epoch;
                    self.gate_attempts.remove("workspace_verification");
                }
                self.record_evidence(
                    if verifies_mutation {
                        ContractEvidenceKind::Verification
                    } else {
                        ContractEvidenceKind::Read
                    },
                    tool_name,
                    input_json,
                );
            }
            Some(ToolRisk::ExecutesProcess) => {
                let verifies_mutation = self.mutation_epoch > self.verified_mutation_epoch
                    && process_input_looks_like_verification(input_json);
                if verifies_mutation {
                    self.verified_mutation_epoch = self.mutation_epoch;
                    self.gate_attempts.remove("workspace_verification");
                }
                self.record_evidence(
                    if verifies_mutation {
                        ContractEvidenceKind::Verification
                    } else {
                        ContractEvidenceKind::OtherTool
                    },
                    tool_name,
                    input_json,
                );
            }
            _ => self.record_evidence(ContractEvidenceKind::OtherTool, tool_name, input_json),
        }
    }

    pub fn completion_instruction(
        &mut self,
        verification_required: bool,
        tools: &[ToolSpec],
    ) -> Result<Option<String>, AgentFailure> {
        self.completion_instruction_with_requirement(verification_required, tools)
    }

    pub fn completion_instruction_for_task(
        &mut self,
        tools: &[ToolSpec],
    ) -> Result<Option<String>, AgentFailure> {
        self.completion_instruction_with_requirement(
            self.workspace_verification_policy.is_required(),
            tools,
        )
    }

    fn completion_instruction_with_requirement(
        &mut self,
        verification_required: bool,
        tools: &[ToolSpec],
    ) -> Result<Option<String>, AgentFailure> {
        let available_tools = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<BTreeSet<_>>();

        if let Some(tool_name) = self
            .required_tool_successes
            .iter()
            .find(|tool_name| !self.successful_tools.contains(*tool_name))
            .cloned()
        {
            if !available_tools.contains(tool_name.as_str()) {
                return Err(AgentFailure::contract(
                    "required_tool_unavailable",
                    format!("task contract requires unavailable tool `{tool_name}`"),
                ));
            }
            self.claim_gate(format!("tool:{tool_name}"))?;
            return Ok(Some(format!(
                "The task contract is not satisfied yet. Use `{tool_name}` successfully before finishing. Do not substitute another implementation or merely describe the intended result."
            )));
        }

        if let Some(tool_name) = self
            .prompt_required_tool_successes
            .iter()
            .find(|tool_name| !self.prompt_successful_tools.contains(*tool_name))
            .cloned()
        {
            if !available_tools.contains(tool_name.as_str()) {
                return Err(AgentFailure::contract(
                    "required_tool_unavailable",
                    format!("task contract requires unavailable tool `{tool_name}`"),
                ));
            }
            self.claim_gate(format!(
                "prompt_tool:{}:{tool_name}",
                self.prompt_requirement_epoch
            ))?;
            return Ok(Some(format!(
                "The task contract is not satisfied yet. Use `{tool_name}` successfully before finishing. Do not substitute another implementation or merely describe the intended result."
            )));
        }

        if let Some((requirement_id, alternatives)) = self
            .required_any_tool_successes
            .iter()
            .find(|(_, alternatives)| alternatives.is_disjoint(&self.successful_tools))
            .map(|(requirement_id, alternatives)| (requirement_id.clone(), alternatives.clone()))
        {
            let available_alternatives = alternatives
                .iter()
                .filter(|tool| available_tools.contains(tool.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if available_alternatives.is_empty() {
                return Err(AgentFailure::contract(
                    "required_tool_group_unavailable",
                    format!(
                        "task contract requirement `{requirement_id}` has no available tool alternatives"
                    ),
                ));
            }
            self.claim_gate(format!("any_tool:{requirement_id}"))?;
            let alternatives = available_alternatives
                .iter()
                .map(|tool| format!("`{tool}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(Some(format!(
                "The task contract is not satisfied yet. Requirement `{requirement_id}` needs substantive evidence from at least one of: {alternatives}. Use one successfully before finishing; discovery-only observations do not satisfy this requirement."
            )));
        }

        if let Some((requirement_id, alternatives)) = self
            .prompt_required_any_tool_successes
            .iter()
            .find(|(_, alternatives)| alternatives.is_disjoint(&self.prompt_successful_tools))
            .map(|(requirement_id, alternatives)| (requirement_id.clone(), alternatives.clone()))
        {
            let available_alternatives = alternatives
                .iter()
                .filter(|tool| available_tools.contains(tool.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if available_alternatives.is_empty() {
                return Err(AgentFailure::contract(
                    "required_prompt_tool_group_unavailable",
                    format!(
                        "task contract requirement `{requirement_id}` has no available tool alternatives"
                    ),
                ));
            }
            self.claim_gate(format!(
                "prompt_any_tool:{}:{requirement_id}",
                self.prompt_requirement_epoch
            ))?;
            let alternatives = available_alternatives
                .iter()
                .map(|tool| format!("`{tool}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(Some(format!(
                "The current request is not complete yet. Requirement `{requirement_id}` needs one successful action from: {alternatives}. Evidence from an earlier user objective does not satisfy it."
            )));
        }

        if let Some((requirement_id, requirement)) = self
            .prompt_evidence_requirements
            .iter()
            .find(|(_, requirement)| requirement.receipt.is_none())
            .map(|(id, requirement)| (id.clone(), requirement.clone()))
        {
            let available_alternatives = requirement
                .tools
                .iter()
                .filter(|tool| available_tools.contains(tool.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if available_alternatives.is_empty() {
                return Err(AgentFailure::contract(
                    "required_evidence_unavailable",
                    format!(
                        "task contract requirement `{requirement_id}` has no available substantive evidence tool"
                    ),
                ));
            }
            self.claim_gate(format!(
                "prompt_evidence:{}:{requirement_id}",
                self.prompt_evidence_epoch
            ))?;
            let alternatives = available_alternatives
                .iter()
                .map(|tool| format!("`{tool}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(Some(format!(
                "The current request requires grounded evidence before completion. Requirement `{requirement_id}` needs a successful substantive observation from at least one of: {alternatives}. Discovery-only results, failed calls, and evidence from an earlier user objective do not satisfy it."
            )));
        }

        if !self.pending_interactions.is_empty() {
            let requirements =
                interaction_requirements(&self.pending_interactions, &available_tools);
            if !requirements.is_empty() {
                self.claim_gate("interaction_verification".to_string())?;
                return Ok(Some(format!(
                    "The task performed interactive actions whose postconditions have not been observed. Before finishing, verify {}. Compare the fresh observation with the requested outcome; retry or replan if it is wrong.",
                    requirements.join("; ")
                )));
            }
        }

        if verification_required
            && self.mutation_epoch > 0
            && !self.latest_mutation_verified()
            && tools.iter().any(tool_can_verify_workspace_change)
        {
            self.claim_gate("workspace_verification".to_string())?;
            return Ok(Some(
                "The task changed the workspace but has no relevant post-change verification evidence. Use the narrowest applicable test, build, lint, diff, or direct read-back of the changed target before finishing. Unrelated tool success is not verification."
                    .to_string(),
            ));
        }

        Ok(None)
    }

    fn claim_gate(&mut self, key: String) -> Result<(), AgentFailure> {
        let attempts = self.gate_attempts.entry(key.clone()).or_default();
        if *attempts >= MAX_COMPLETION_GATE_ATTEMPTS {
            return Err(AgentFailure::contract(
                "task_contract_unsatisfied",
                format!(
                    "task contract requirement `{key}` remained unsatisfied after {attempts} repair attempts"
                ),
            ));
        }
        *attempts += 1;
        Ok(())
    }

    fn record_evidence(&mut self, kind: ContractEvidenceKind, source: &str, input: &str) {
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.evidence.push(ContractEvidence {
            sequence: self.next_sequence,
            kind,
            source: source.to_string(),
            input_fingerprint: fingerprint(input),
        });
        if self.evidence.len() > MAX_CONTRACT_EVIDENCE {
            let excess = self.evidence.len() - MAX_CONTRACT_EVIDENCE;
            self.evidence.drain(..excess);
        }
    }
}

fn fingerprint(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn substantive_observation(observation: &str) -> bool {
    let observation = observation.trim();
    if observation.is_empty() {
        return false;
    }
    if let Some((_, output)) = observation.split_once("output=") {
        return !output.trim().is_empty();
    }
    true
}

fn bounded_grounding_excerpt(observation: &str) -> String {
    let observation = observation.trim();
    let character_count = observation.chars().count();
    if character_count <= MAX_GROUNDING_EXCERPT_CHARS {
        return observation.to_string();
    }
    let tail_chars = MAX_GROUNDING_EXCERPT_CHARS / 4;
    let head_chars = MAX_GROUNDING_EXCERPT_CHARS - tail_chars;
    let head = observation.chars().take(head_chars).collect::<String>();
    let tail = observation
        .chars()
        .skip(character_count - tail_chars)
        .collect::<String>();
    format!("{head}\n...[grounding observation truncated]...\n{tail}")
}

fn structured_targets(input_json: &str) -> BTreeSet<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(input_json) else {
        return BTreeSet::new();
    };
    let mut targets = BTreeSet::new();
    collect_targets(&value, None, &mut targets);
    targets
}

fn collect_targets(value: &serde_json::Value, key: Option<&str>, targets: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                collect_targets(value, Some(key), targets);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_targets(value, key, targets);
            }
        }
        serde_json::Value::String(value)
            if key.is_some_and(|key| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "path" | "file" | "file_path" | "output" | "output_path" | "directory"
                )
            }) =>
        {
            let normalized = value.trim().trim_end_matches('/').to_string();
            if !normalized.is_empty() {
                targets.insert(normalized);
            }
        }
        _ => {}
    }
}

fn verification_targets_match(mutation_targets: &BTreeSet<String>, input_json: &str) -> bool {
    if mutation_targets.is_empty() {
        return false;
    }
    let verification_targets = structured_targets(input_json);
    mutation_targets.iter().any(|mutation| {
        verification_targets.iter().any(|verification| {
            mutation == verification
                || mutation.starts_with(&format!("{verification}/"))
                || verification.starts_with(&format!("{mutation}/"))
        })
    })
}

fn interaction_action(tool_name: &str) -> Option<(InteractionSurface, &str)> {
    match tool_name {
        "browser.open" | "browser.click" | "browser.type" | "browser.scroll"
        | "browser.select_tab" => Some((InteractionSurface::Browser, tool_name)),
        "computer.click" | "computer.type" | "computer.key" | "computer.scroll" => {
            Some((InteractionSurface::Computer, tool_name))
        }
        _ => None,
    }
}

fn interaction_observation(tool_name: &str) -> Option<InteractionSurface> {
    match tool_name {
        "browser.extract_text" | "browser.capture" | "browser.tabs" => {
            Some(InteractionSurface::Browser)
        }
        "computer.screenshot" => Some(InteractionSurface::Computer),
        _ => None,
    }
}

fn interaction_observation_verifies(action_tool: &str, observation_tool: &str) -> bool {
    match action_tool {
        "browser.open" | "browser.select_tab" => matches!(
            observation_tool,
            "browser.extract_text" | "browser.capture" | "browser.tabs"
        ),
        "browser.click" | "browser.type" | "browser.scroll" => {
            matches!(observation_tool, "browser.extract_text" | "browser.capture")
        }
        "computer.click" | "computer.type" | "computer.key" | "computer.scroll" => {
            observation_tool == "computer.screenshot"
        }
        _ => false,
    }
}

fn interaction_requirements(
    pending: &BTreeMap<InteractionSurface, String>,
    available: &BTreeSet<&str>,
) -> Vec<String> {
    pending
        .iter()
        .filter_map(|(surface, action)| {
            let candidates = interaction_observers(*surface, action);
            let observers = candidates
                .iter()
                .copied()
                .filter(|candidate| available.contains(candidate))
                .collect::<Vec<_>>();
            (!observers.is_empty()).then(|| {
                format!(
                    "{} action `{action}` with {}",
                    surface.label(),
                    observers
                        .iter()
                        .map(|tool| format!("`{tool}`"))
                        .collect::<Vec<_>>()
                        .join(" or ")
                )
            })
        })
        .collect()
}

fn interaction_observers(surface: InteractionSurface, action: &str) -> &'static [&'static str] {
    match surface {
        InteractionSurface::Browser if matches!(action, "browser.open" | "browser.select_tab") => {
            &["browser.capture", "browser.extract_text", "browser.tabs"]
        }
        InteractionSurface::Browser => &["browser.capture", "browser.extract_text"],
        InteractionSurface::Computer => &["computer.screenshot"],
    }
}

fn inferred_builtin_tool_risk(tool_name: &str) -> Option<&'static ToolRisk> {
    static READ_ONLY: ToolRisk = ToolRisk::ReadOnly;
    static WRITES_WORKSPACE: ToolRisk = ToolRisk::WritesWorkspace;
    static EXECUTES_PROCESS: ToolRisk = ToolRisk::ExecutesProcess;
    match tool_name {
        "file.read" | "file.read_many" | "file.list" | "file.search" => Some(&READ_ONLY),
        "file.write" => Some(&WRITES_WORKSPACE),
        "shell.run" => Some(&EXECUTES_PROCESS),
        _ => None,
    }
}

fn process_input_looks_like_verification(input_json: &str) -> bool {
    let normalized = input_json.to_ascii_lowercase();
    [
        " test",
        "test ",
        "check",
        "build",
        "lint",
        "verify",
        "pytest",
        "vitest",
        "jest",
        "cargo test",
        "cargo check",
        "swift test",
        "go test",
        "git diff",
        "git status",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn tool_can_verify_workspace_change(tool: &ToolSpec) -> bool {
    matches!(tool.risk, ToolRisk::ReadOnly | ToolRisk::ExecutesProcess)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, risk: ToolRisk) -> ToolSpec {
        ToolSpec::builtin(name, "test", "test", risk, r#"{"type":"object"}"#)
    }

    #[test]
    fn required_tool_is_a_hard_completion_contract() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("image.generate");
        let tools = vec![tool("image.generate", ToolRisk::UsesNetwork)];

        assert!(contract
            .completion_instruction_for_task(&tools)
            .unwrap()
            .unwrap()
            .contains("image.generate"));
        contract.record_tool_outcome(
            "image.generate",
            r#"{"prompt":"city"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn prompt_requirement_is_removed_when_a_new_epoch_no_longer_needs_it() {
        let mut contract = AgentTaskContract::default();
        let tools = vec![tool("image.generate", ToolRisk::UsesNetwork)];
        contract.replace_prompt_required_tool_successes(0, ["image.generate"]);

        contract.record_tool_outcome(
            "image.generate",
            r#"{"prompt":"city"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));

        contract.replace_prompt_required_tool_successes(1, std::iter::empty::<&str>());

        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn prompt_requirement_does_not_reuse_success_from_an_older_epoch() {
        let mut contract = AgentTaskContract::default();
        let tools = vec![tool("image.generate", ToolRisk::UsesNetwork)];
        contract.replace_prompt_required_tool_successes(3, ["image.generate"]);
        contract.record_tool_outcome(
            "image.generate",
            r#"{"prompt":"first image"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        assert!(contract.required_tool_satisfied("image.generate"));

        contract.replace_prompt_required_tool_successes(4, ["image.generate"]);

        assert!(!contract.required_tool_satisfied("image.generate"));
        assert!(contract
            .model_context_for_task(&tools)
            .expect("new epoch should expose the renewed requirement")
            .contains("image.generate"));
        assert!(contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate evaluates")
            .expect("new epoch requires new success evidence")
            .contains("image.generate"));

        contract.record_tool_outcome(
            "image.generate",
            r#"{"prompt":"second image"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        contract.replace_prompt_required_tool_successes(4, ["image.generate"]);
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn rebuilding_prompt_requirements_preserves_run_wide_contract_state() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("file.read");
        contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        contract.record_tool_outcome("computer.click", "{}", &ToolOutcomeStatus::Succeeded, None);
        let evidence = contract.evidence().to_vec();

        contract.replace_prompt_required_tool_successes(7, ["image.generate"]);
        contract.replace_prompt_required_tool_successes(8, std::iter::empty::<&str>());

        assert_eq!(
            contract.workspace_verification_policy(),
            WorkspaceVerificationPolicy::RequiredAfterMutation
        );
        assert_eq!(contract.successful_mutations(), 1);
        assert!(!contract.latest_mutation_verified());
        assert!(contract.required_tool_satisfied("file.read"));
        assert_eq!(
            contract.pending_interactions()[&InteractionSurface::Computer],
            "computer.click"
        );
        assert_eq!(contract.evidence(), evidence);
    }

    #[test]
    fn legacy_snapshot_without_prompt_epoch_fields_remains_compatible() {
        let encoded = r#"{
            "requiredToolSuccesses":["image.generate"],
            "successfulTools":["image.generate"]
        }"#;
        let mut contract = serde_json::from_str::<AgentTaskContract>(encoded)
            .expect("legacy task contract should decode");
        let tools = vec![tool("image.generate", ToolRisk::UsesNetwork)];

        assert!(contract.required_tool_satisfied("image.generate"));
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));

        contract.replace_prompt_required_tool_successes(1, ["image.generate"]);
        assert!(!contract.required_tool_satisfied("image.generate"));
        assert!(contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate evaluates")
            .is_some());
    }

    #[test]
    fn any_tool_requirement_accepts_one_successful_alternative() {
        let mut contract = AgentTaskContract::default();
        contract.require_any_tool_success(
            "workspace_content",
            ["file.read", "file.read_many", "file.search"],
        );
        let tools = vec![
            tool("file.read", ToolRisk::ReadOnly),
            tool("file.read_many", ToolRisk::ReadOnly),
            tool("file.search", ToolRisk::ReadOnly),
        ];

        let instruction = contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate")
            .expect("requirement should be active");
        assert!(instruction.contains("workspace_content"));
        assert!(instruction.contains("file.read_many"));

        contract.record_tool_outcome(
            "file.read_many",
            r#"{"paths":["a.md","b.md"]}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn prompt_capability_requirement_is_replaced_at_the_steer_epoch() {
        let mut contract = AgentTaskContract::default();
        let tools = vec![
            tool("browser.open", ToolRisk::UsesNetwork),
            tool("file.write", ToolRisk::WritesWorkspace),
        ];
        contract.replace_prompt_required_any_tool_successes(
            3,
            [(
                "browser_action".to_string(),
                BTreeSet::from(["browser.open".to_string()]),
            )]
            .into_iter()
            .collect(),
        );
        contract.record_tool_outcome(
            "browser.open",
            r#"{"url":"https://example.com"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));

        contract.replace_prompt_required_any_tool_successes(
            4,
            [(
                "workspace_effect".to_string(),
                BTreeSet::from(["file.write".to_string()]),
            )]
            .into_iter()
            .collect(),
        );

        let instruction = contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate")
            .expect("new steer needs current evidence");
        assert!(instruction.contains("workspace_effect"));
        assert!(instruction.contains("file.write"));
        assert!(!instruction.contains("browser.open"));
    }

    #[test]
    fn replaying_the_same_prompt_capability_epoch_preserves_success() {
        let mut contract = AgentTaskContract::default();
        let requirements = [(
            "effect".to_string(),
            BTreeSet::from(["file.write".to_string()]),
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let tools = vec![tool("file.write", ToolRisk::WritesWorkspace)];
        contract.replace_prompt_required_any_tool_successes(8, requirements.clone());
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"result.txt"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        contract.replace_prompt_required_any_tool_successes(8, requirements);

        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn unrelated_discovery_tool_does_not_satisfy_any_tool_requirement() {
        let mut contract = AgentTaskContract::default();
        contract.require_any_tool_success("workspace_content", ["file.read", "file.read_many"]);
        let tools = vec![
            tool("file.list", ToolRisk::ReadOnly),
            tool("file.read", ToolRisk::ReadOnly),
            tool("file.read_many", ToolRisk::ReadOnly),
        ];

        contract.record_tool_outcome(
            "file.list",
            r#"{"path":""}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate")
            .is_some());
    }

    #[test]
    fn prompt_evidence_is_scoped_to_the_active_epoch() {
        let mut contract = AgentTaskContract::default();
        let tools = vec![tool("file.read", ToolRisk::ReadOnly)];
        contract.replace_prompt_evidence_requirement(3, Some("workspace_grounding"), ["file.read"]);
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(contract.record_prompt_tool_evidence_observation_at(
            3,
            "file.read",
            "file.read",
            r#"{"path":"README.md"}"#,
            "tool=file.read\nstatus=succeeded\noutput=\nrepository readme",
        ));
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));

        contract.replace_prompt_evidence_requirement(3, Some("workspace_grounding"), ["file.read"]);
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));

        contract.replace_prompt_evidence_requirement(4, Some("workspace_grounding"), ["file.read"]);
        assert!(contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate evaluates")
            .is_some());

        contract.replace_prompt_evidence_requirement(5, None, std::iter::empty::<&str>());
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn prompt_evidence_rejects_failures_discovery_and_precontract_text_replay() {
        let mut contract = AgentTaskContract::default();
        let tools = vec![
            tool("file.list", ToolRisk::ReadOnly),
            tool("file.read", ToolRisk::ReadOnly),
        ];
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"forged.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        contract.replace_prompt_evidence_requirement(1, Some("workspace_grounding"), ["file.read"]);

        for (tool_name, status) in [
            ("file.list", ToolOutcomeStatus::Succeeded),
            ("file.read", ToolOutcomeStatus::Failed),
            ("file.read", ToolOutcomeStatus::Denied),
            ("file.read", ToolOutcomeStatus::Cancelled),
        ] {
            contract.record_tool_outcome(
                tool_name,
                r#"{"path":"README.md"}"#,
                &status,
                Some(&ToolRisk::ReadOnly),
            );
        }
        assert!(contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate evaluates")
            .is_some());

        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(!contract.record_prompt_tool_evidence_observation_at(
            1,
            "file.read",
            "file.read",
            r#"{"path":"README.md"}"#,
            "tool=file.read\nstatus=succeeded\noutput=\n   ",
        ));
        assert!(contract.record_prompt_tool_evidence_observation_at(
            1,
            "file.read",
            "file.read",
            r#"{"path":"README.md"}"#,
            "tool=file.read\nstatus=succeeded\noutput=\nrepository readme",
        ));
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn trusted_prompt_evidence_receipt_round_trips_without_a_tool_alternative() {
        let mut contract = AgentTaskContract::default();
        contract.replace_prompt_evidence_requirement(
            9,
            Some("workspace_grounding"),
            std::iter::empty::<&str>(),
        );
        let failure = contract
            .completion_instruction_for_task(&[])
            .expect_err("missing evidence capability must fail closed");
        assert_eq!(failure.code, "required_evidence_unavailable");

        assert!(!contract.record_prompt_evidence_for_requirement_at(
            8,
            "workspace_grounding",
            "stale_knowledge_context",
            "selected_count=3",
            "grounded snippets",
        ));
        assert!(!contract.record_prompt_tool_evidence_observation_at(
            9,
            "web.search",
            "wrong_domain",
            "receipt",
            "search result",
        ));
        assert!(contract.record_prompt_evidence_for_requirement_at(
            9,
            "workspace_grounding",
            "knowledge_context",
            "selected_count=3",
            "grounded snippets",
        ));
        let encoded = serde_json::to_string(&contract).expect("contract serializes");
        let mut restored =
            serde_json::from_str::<AgentTaskContract>(&encoded).expect("contract should restore");
        let restored_failure = restored
            .completion_instruction_for_task(&[])
            .expect_err("runtime-only evidence must fail closed after persistence");
        assert_eq!(restored_failure.code, "required_evidence_unavailable");
        assert!(restored.evidence().iter().any(|evidence| {
            evidence.kind == ContractEvidenceKind::Grounding
                && evidence.source == "knowledge_context"
        }));
    }

    #[test]
    fn unrelated_read_does_not_verify_a_mutation() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(!contract.latest_mutation_verified());

        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(contract.latest_mutation_verified());
    }

    #[test]
    fn interaction_action_requires_a_matching_fresh_observation() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome("computer.click", "{}", &ToolOutcomeStatus::Succeeded, None);
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(!contract.pending_interactions().is_empty());
        contract.record_tool_outcome(
            "computer.screenshot",
            "{}",
            &ToolOutcomeStatus::Succeeded,
            None,
        );
        assert!(contract.pending_interactions().is_empty());
    }

    #[test]
    fn model_context_exposes_active_obligations_without_claiming_a_gate_attempt() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("image.generate");
        let tools = vec![tool("image.generate", ToolRisk::UsesNetwork)];

        let first = contract
            .model_context_for_task(&tools)
            .expect("required tool should be visible");
        let second = contract
            .model_context_for_task(&tools)
            .expect("rendering should be repeatable");

        assert_eq!(first, second);
        assert!(first.contains("image.generate"));
        assert!(contract.gate_attempts.is_empty());
        assert!(contract.completion_instruction_for_task(&tools).is_ok());
    }

    #[test]
    fn model_context_tracks_and_clears_relevant_workspace_verification() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        let tools = vec![tool("file.read", ToolRisk::ReadOnly)];

        assert!(contract.model_context_for_task(&tools).is_none());
        contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );

        let pending = contract
            .model_context_for_task(&tools)
            .expect("mutation should require verification");
        assert!(pending.contains("src/main.rs"));
        contract.merge_workspace_verification_policy(WorkspaceVerificationPolicy::NotRequired);
        assert_eq!(
            contract.workspace_verification_policy(),
            WorkspaceVerificationPolicy::RequiredAfterMutation
        );

        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(contract.model_context_for_task(&tools).is_none());
    }

    #[test]
    fn explicit_verification_arguments_preserve_legacy_behavior() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        let tools = vec![tool("file.read", ToolRisk::ReadOnly)];

        assert!(contract.model_context(false, &tools).is_none());
        assert!(contract.model_context(true, &tools).is_some());

        let mut disabled = contract.clone();
        assert_eq!(disabled.completion_instruction(false, &tools), Ok(None));
        assert!(contract
            .completion_instruction(true, &tools)
            .expect("legacy completion gate evaluates")
            .is_some());
    }

    #[test]
    fn model_context_includes_matching_interaction_observers() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            "computer.click",
            r#"{"x":10,"y":20}"#,
            &ToolOutcomeStatus::Succeeded,
            None,
        );
        let tools = vec![tool("computer.screenshot", ToolRisk::ReadOnly)];

        let context = contract
            .model_context_for_task(&tools)
            .expect("fresh observation should be required");
        assert!(context.contains("computer.click"));
        assert!(context.contains("computer.screenshot"));
    }
}
