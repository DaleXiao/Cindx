use super::{
    interaction_observers, tool_can_verify_workspace_change, AgentTaskContract,
    ContractEvidenceKind, OutcomeLedgerShadow, OutcomeObligation, OutcomeObligationKind,
    OutcomePostcondition, OutcomePostconditionKind, OutcomePostconditionStatus,
    OutcomeSatisfaction, OutcomeScope,
};
use crate::{
    task_state_lineage::text_fingerprint, AdaptiveLoopDisposition, PreparedTaskState,
    PromptToolRequirement,
};
use agent_core::ToolSpec;
use serde::Serialize;
use std::collections::BTreeSet;

pub const COGNITIVE_STATE_SCHEMA: &str = "cindx.agent.cognitive-state.v1";
pub const COGNITIVE_STATE_MAX_BYTES: usize = 8 * 1024;

const MAX_COGNITIVE_OBLIGATIONS: usize = 8;
const MAX_COGNITIVE_EVIDENCE: usize = 6;
const MAX_COGNITIVE_POSTCONDITIONS: usize = 6;
const MAX_COGNITIVE_ACTIONS: usize = 8;
const MAX_COGNITIVE_ACTION_TOOLS: usize = 6;
const MAX_COGNITIVE_TOOL_NAME_BYTES: usize = 128;
const MAX_COGNITIVE_TARGETS: usize = 8;

/// A bounded, advisory projection of the task contract for one prepared epoch.
///
/// This state never owns task facts. It contains only fingerprints, typed
/// obligations, evidence references, and postconditions copied from the
/// authoritative task contract and outcome ledger. In particular, it stores no
/// transcript text, tool input/output, or model reasoning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCognitiveState {
    schema: &'static str,
    steer_epoch: u64,
    contract_epoch: u64,
    focus: AgentCognitiveFocus,
    adaptive_disposition: AdaptiveLoopDisposition,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    actions: Vec<CognitiveAction>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    verification_targets: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pending_obligations: Vec<CognitiveObligation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocked_obligations: Vec<CognitiveBlockedObligation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pending_postconditions: Vec<CognitivePostcondition>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<CognitiveEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncation: Option<CognitiveTruncation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCognitiveFocus {
    Answer,
    Act,
    GatherEvidence,
    Verify,
    Replan,
    ReportBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CognitiveActionKind {
    Act,
    GatherEvidence,
    Verify,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CognitiveAction {
    kind: CognitiveActionKind,
    one_of: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CognitiveObligation {
    id: String,
    kind: OutcomeObligationKind,
    scope: OutcomeScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CognitiveBlockedObligation {
    id: String,
    kind: OutcomeObligationKind,
    scope: OutcomeScope,
    blocker_kind: super::AgentActionDenialKind,
    blocker_code: String,
    evidence_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CognitivePostcondition {
    id: String,
    kind: OutcomePostconditionKind,
    action_sequence: u64,
    action_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CognitiveEvidence {
    sequence: u64,
    kind: ContractEvidenceKind,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CognitiveTruncation {
    actions: usize,
    targets: usize,
    obligations: usize,
    postconditions: usize,
    evidence: usize,
    source_ledger: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CognitiveProgressProjection<'a> {
    schema: &'static str,
    steer_epoch: u64,
    contract_epoch: u64,
    actions: &'a [CognitiveAction],
    verification_targets: &'a [String],
    pending_obligations: &'a [CognitiveObligation],
    blocked_obligations: &'a [CognitiveBlockedObligation],
    pending_postconditions: &'a [CognitivePostcondition],
    evidence: &'a [CognitiveEvidence],
    truncation: &'a Option<CognitiveTruncation>,
}

impl AgentCognitiveState {
    /// Builds a side-effect-free projection of the current authoritative state.
    pub fn project(prepared: &PreparedTaskState, contract: &AgentTaskContract) -> Self {
        Self::project_with_adaptive(prepared, contract, AdaptiveLoopDisposition::Continue)
    }

    pub fn project_with_tools(
        prepared: &PreparedTaskState,
        contract: &AgentTaskContract,
        tools: &[ToolSpec],
    ) -> Self {
        Self::project_with_tools_and_adaptive(
            prepared,
            contract,
            tools,
            AdaptiveLoopDisposition::Continue,
        )
    }

    pub fn project_with_adaptive(
        prepared: &PreparedTaskState,
        contract: &AgentTaskContract,
        adaptive_disposition: AdaptiveLoopDisposition,
    ) -> Self {
        Self::project_with_tools_and_adaptive(prepared, contract, &[], adaptive_disposition)
    }

    pub fn project_with_tools_and_adaptive(
        prepared: &PreparedTaskState,
        contract: &AgentTaskContract,
        tools: &[ToolSpec],
        adaptive_disposition: AdaptiveLoopDisposition,
    ) -> Self {
        let ledger = contract.outcome_ledger_shadow(prepared.steer_epoch());
        let mut state = Self::from_outcome_ledger(prepared, &ledger);
        let (actions, omitted_actions) = cognitive_actions(contract, tools);
        state.actions = actions;
        let verification_pending = contract.workspace_verification_policy.is_required()
            && contract.mutation_epoch > 0
            && !contract.latest_mutation_verified();
        let verification_targets = if verification_pending {
            contract
                .mutation_targets
                .iter()
                .take(MAX_COGNITIVE_TARGETS)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let omitted_targets = if verification_pending {
            contract
                .mutation_targets
                .len()
                .saturating_sub(verification_targets.len())
        } else {
            0
        };
        state.verification_targets = verification_targets;
        if omitted_actions > 0 || omitted_targets > 0 {
            let truncation = state.truncation.get_or_insert(CognitiveTruncation {
                actions: 0,
                targets: 0,
                obligations: 0,
                postconditions: 0,
                evidence: 0,
                source_ledger: false,
            });
            truncation.actions = omitted_actions;
            truncation.targets = omitted_targets;
        }
        state.adaptive_disposition = adaptive_disposition;
        state.focus = match adaptive_disposition {
            AdaptiveLoopDisposition::Continue => state.focus,
            AdaptiveLoopDisposition::ReplanOnce => AgentCognitiveFocus::Replan,
            AdaptiveLoopDisposition::CommitTerminalResult => {
                if state.focus == AgentCognitiveFocus::ReportBlocked {
                    AgentCognitiveFocus::ReportBlocked
                } else {
                    AgentCognitiveFocus::Answer
                }
            }
        };
        state
    }

    pub(crate) fn project_with_verification_requirement(
        prepared: &PreparedTaskState,
        contract: &AgentTaskContract,
        tools: &[ToolSpec],
        verification_required: bool,
        adaptive_disposition: AdaptiveLoopDisposition,
    ) -> Self {
        let mut projected_contract = contract.clone();
        projected_contract.workspace_verification_policy = if verification_required {
            super::WorkspaceVerificationPolicy::RequiredAfterMutation
        } else {
            super::WorkspaceVerificationPolicy::NotRequired
        };
        Self::project_with_tools_and_adaptive(
            prepared,
            &projected_contract,
            tools,
            adaptive_disposition,
        )
    }

    pub fn focus(&self) -> AgentCognitiveFocus {
        self.focus
    }

    /// Serializes the projection only when it satisfies the prompt-size cap.
    pub fn to_bounded_json(&self) -> Option<String> {
        let encoded = serde_json::to_string(self).ok()?;
        (encoded.len() <= COGNITIVE_STATE_MAX_BYTES).then_some(encoded)
    }

    /// Stable digest for comparing authoritative progress between preparations.
    /// The digest is an observation cursor, not a completion or permission fact.
    pub fn progress_fingerprint(&self) -> Option<String> {
        serde_json::to_string(&CognitiveProgressProjection {
            schema: self.schema,
            steer_epoch: self.steer_epoch,
            contract_epoch: self.contract_epoch,
            actions: &self.actions,
            verification_targets: &self.verification_targets,
            pending_obligations: &self.pending_obligations,
            blocked_obligations: &self.blocked_obligations,
            pending_postconditions: &self.pending_postconditions,
            evidence: &self.evidence,
            truncation: &self.truncation,
        })
        .ok()
        .map(|encoded| text_fingerprint(&encoded))
    }

    fn from_outcome_ledger(prepared: &PreparedTaskState, ledger: &OutcomeLedgerShadow) -> Self {
        let current_obligations = ledger
            .obligations
            .iter()
            .filter(|obligation| obligation_is_current(obligation, prepared.contract_epoch()))
            .collect::<Vec<_>>();
        let pending_count = current_obligations
            .iter()
            .filter(|obligation| obligation.satisfaction == OutcomeSatisfaction::Pending)
            .count();
        let blocked_count = current_obligations
            .iter()
            .filter(|obligation| obligation.satisfaction == OutcomeSatisfaction::Blocked)
            .count();
        let required_pending_postconditions = ledger
            .postconditions
            .iter()
            .filter(|postcondition| {
                postcondition.required
                    && postcondition.status == OutcomePostconditionStatus::Pending
            })
            .collect::<Vec<_>>();
        let postcondition_start = required_pending_postconditions
            .len()
            .saturating_sub(MAX_COGNITIVE_POSTCONDITIONS);
        let pending_postconditions = required_pending_postconditions[postcondition_start..]
            .iter()
            .map(cognitive_postcondition)
            .collect::<Vec<_>>();

        let focus = select_focus(
            &current_obligations,
            required_pending_postconditions.len(),
            prepared.completion_intent().tool_requirement,
        );

        let mut blocked_obligations = Vec::new();
        let mut pending_obligations = Vec::new();
        for obligation in current_obligations
            .iter()
            .copied()
            .filter(|obligation| obligation.satisfaction == OutcomeSatisfaction::Blocked)
        {
            if blocked_obligations.len() + pending_obligations.len() >= MAX_COGNITIVE_OBLIGATIONS {
                break;
            }
            if let (Some(blocker), Some(evidence_sequence)) =
                (&obligation.blocker, obligation.evidence_sequence)
            {
                blocked_obligations.push(CognitiveBlockedObligation {
                    id: obligation.id.clone(),
                    kind: obligation.kind,
                    scope: obligation.scope,
                    blocker_kind: blocker.kind,
                    blocker_code: blocker.code.clone(),
                    evidence_sequence,
                });
            }
        }
        for obligation in current_obligations
            .iter()
            .copied()
            .filter(|obligation| obligation.satisfaction == OutcomeSatisfaction::Pending)
        {
            if blocked_obligations.len() + pending_obligations.len() >= MAX_COGNITIVE_OBLIGATIONS {
                break;
            }
            pending_obligations.push(CognitiveObligation {
                id: obligation.id.clone(),
                kind: obligation.kind,
                scope: obligation.scope,
            });
        }

        let mut relevant_sequences = current_obligations
            .iter()
            .filter_map(|obligation| obligation.evidence_sequence)
            .collect::<BTreeSet<_>>();
        relevant_sequences.extend(
            required_pending_postconditions
                .iter()
                .map(|postcondition| postcondition.action_sequence),
        );
        let relevant_evidence = ledger
            .evidence
            .iter()
            .filter(|evidence| relevant_sequences.contains(&evidence.sequence))
            .collect::<Vec<_>>();
        let evidence_start = relevant_evidence
            .len()
            .saturating_sub(MAX_COGNITIVE_EVIDENCE);
        let evidence = relevant_evidence[evidence_start..]
            .iter()
            .map(|evidence| CognitiveEvidence {
                sequence: evidence.sequence,
                kind: evidence.kind,
                source: evidence.source.clone(),
            })
            .collect::<Vec<_>>();

        let projected_obligations = blocked_obligations.len() + pending_obligations.len();
        let unresolved_obligations = blocked_count + pending_count;
        let source_ledger_truncated = ledger.truncation.obligations > 0
            || ledger.truncation.evidence > 0
            || ledger.truncation.postconditions > 0;

        Self {
            schema: COGNITIVE_STATE_SCHEMA,
            steer_epoch: prepared.steer_epoch(),
            contract_epoch: prepared.contract_epoch(),
            focus,
            adaptive_disposition: AdaptiveLoopDisposition::Continue,
            actions: Vec::new(),
            verification_targets: Vec::new(),
            pending_obligations,
            blocked_obligations,
            pending_postconditions,
            evidence,
            truncation: (unresolved_obligations > projected_obligations
                || required_pending_postconditions.len() > MAX_COGNITIVE_POSTCONDITIONS
                || relevant_evidence.len() > MAX_COGNITIVE_EVIDENCE
                || source_ledger_truncated)
                .then(|| CognitiveTruncation {
                    actions: 0,
                    targets: 0,
                    obligations: unresolved_obligations.saturating_sub(projected_obligations),
                    postconditions: required_pending_postconditions
                        .len()
                        .saturating_sub(MAX_COGNITIVE_POSTCONDITIONS),
                    evidence: relevant_evidence
                        .len()
                        .saturating_sub(MAX_COGNITIVE_EVIDENCE),
                    source_ledger: source_ledger_truncated,
                }),
        }
    }
}

fn cognitive_actions(
    contract: &AgentTaskContract,
    tools: &[ToolSpec],
) -> (Vec<CognitiveAction>, usize) {
    let available = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .filter(|name| !name.is_empty() && name.len() <= MAX_COGNITIVE_TOOL_NAME_BYTES)
        .collect::<BTreeSet<_>>();
    let mut candidates = Vec::new();

    for tool_name in contract
        .required_tool_successes
        .iter()
        .filter(|name| !contract.successful_tools.contains(*name))
        .chain(
            contract
                .prompt_required_tool_successes
                .iter()
                .filter(|name| !contract.prompt_successful_tools.contains(*name)),
        )
    {
        if available.contains(tool_name.as_str())
            && contract
                .action_denial_for_tools([tool_name.as_str()])
                .is_none()
        {
            push_action(
                &mut candidates,
                CognitiveActionKind::Act,
                vec![tool_name.clone()],
            );
        }
    }

    for alternatives in contract
        .required_any_tool_successes
        .iter()
        .filter(|(_, alternatives)| alternatives.is_disjoint(&contract.successful_tools))
        .map(|(_, alternatives)| alternatives)
        .chain(
            contract
                .prompt_required_any_tool_successes
                .iter()
                .filter(|(_, alternatives)| {
                    alternatives.is_disjoint(&contract.prompt_successful_tools)
                })
                .map(|(_, alternatives)| alternatives),
        )
    {
        if contract
            .action_denial_for_tools(alternatives.iter().map(String::as_str))
            .is_none()
        {
            push_action(
                &mut candidates,
                CognitiveActionKind::Act,
                available_action_tools(alternatives, &available),
            );
        }
    }

    for requirement in contract
        .prompt_evidence_requirements
        .values()
        .filter(|requirement| requirement.receipt.is_none())
    {
        if contract
            .action_denial_for_tools(requirement.tools.iter().map(String::as_str))
            .is_none()
        {
            push_action(
                &mut candidates,
                CognitiveActionKind::GatherEvidence,
                available_action_tools(&requirement.tools, &available),
            );
        }
    }

    if contract.workspace_verification_policy.is_required()
        && contract.mutation_epoch > 0
        && !contract.latest_mutation_verified()
    {
        push_action(
            &mut candidates,
            CognitiveActionKind::Verify,
            tools
                .iter()
                .filter(|tool| {
                    available.contains(tool.name.as_str()) && tool_can_verify_workspace_change(tool)
                })
                .map(|tool| tool.name.clone())
                .take(MAX_COGNITIVE_ACTION_TOOLS)
                .collect(),
        );
    }

    for (surface, action) in &contract.pending_interactions {
        push_action(
            &mut candidates,
            CognitiveActionKind::Verify,
            interaction_observers(*surface, action)
                .iter()
                .filter(|name| available.contains(**name))
                .map(|name| (*name).to_string())
                .take(MAX_COGNITIVE_ACTION_TOOLS)
                .collect(),
        );
    }

    let omitted = candidates.len().saturating_sub(MAX_COGNITIVE_ACTIONS);
    candidates.truncate(MAX_COGNITIVE_ACTIONS);
    (candidates, omitted)
}

fn available_action_tools(
    alternatives: &BTreeSet<String>,
    available: &BTreeSet<&str>,
) -> Vec<String> {
    alternatives
        .iter()
        .filter(|name| available.contains(name.as_str()))
        .take(MAX_COGNITIVE_ACTION_TOOLS)
        .cloned()
        .collect()
}

fn push_action(actions: &mut Vec<CognitiveAction>, kind: CognitiveActionKind, one_of: Vec<String>) {
    if one_of.is_empty()
        || actions
            .iter()
            .any(|action| action.kind == kind && action.one_of == one_of)
    {
        return;
    }
    actions.push(CognitiveAction { kind, one_of });
}

fn obligation_is_current(obligation: &OutcomeObligation, contract_epoch: u64) -> bool {
    match obligation.scope {
        OutcomeScope::Run => true,
        OutcomeScope::Steer => obligation.steer_epoch == Some(contract_epoch),
    }
}

fn cognitive_postcondition(postcondition: &&OutcomePostcondition) -> CognitivePostcondition {
    CognitivePostcondition {
        id: postcondition.id.clone(),
        kind: postcondition.kind,
        action_sequence: postcondition.action_sequence,
        action_source: postcondition.action_source.clone(),
    }
}

fn select_focus(
    obligations: &[&OutcomeObligation],
    pending_postconditions: usize,
    tool_requirement: PromptToolRequirement,
) -> AgentCognitiveFocus {
    let pending = obligations
        .iter()
        .copied()
        .filter(|obligation| obligation.satisfaction == OutcomeSatisfaction::Pending)
        .collect::<Vec<_>>();
    let blocked = obligations
        .iter()
        .any(|obligation| obligation.satisfaction == OutcomeSatisfaction::Blocked);

    if blocked {
        return if pending.is_empty() && pending_postconditions == 0 {
            AgentCognitiveFocus::ReportBlocked
        } else {
            AgentCognitiveFocus::Replan
        };
    }
    if pending_postconditions > 0
        || pending.iter().any(|obligation| {
            matches!(
                obligation.kind,
                OutcomeObligationKind::WorkspaceVerification
                    | OutcomeObligationKind::InteractionObservation
            )
        })
    {
        return AgentCognitiveFocus::Verify;
    }
    if pending
        .iter()
        .any(|obligation| obligation.kind == OutcomeObligationKind::Grounding)
        || (!pending.is_empty() && tool_requirement == PromptToolRequirement::ReadOnly)
    {
        return AgentCognitiveFocus::GatherEvidence;
    }
    if !pending.is_empty() {
        return AgentCognitiveFocus::Act;
    }
    AgentCognitiveFocus::Answer
}

impl super::AgentTaskContract {
    /// Anchor-matched absence is an observed workspace fact, so prompt-scoped
    /// any-tool obligations accept it instead of an impossible successful read.
    pub(crate) fn has_anchor_matched_absence_for(&self, tools: &BTreeSet<String>) -> bool {
        self.prompt_evidence_requirements
            .values()
            .any(|requirement| {
                requirement
                    .receipt
                    .as_ref()
                    .is_some_and(|receipt| receipt.absent && tools.contains(&receipt.source))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adaptive_loop::{MAX_ADAPTIVE_NO_GAIN_COUNT, MAX_RECENT_SEMANTIC_ACTIONS},
        AdaptiveLoopCursor, AgentActionDenialFeedback, PromptCompletionIntent,
        WorkspaceVerificationPolicy,
    };
    use agent_core::{
        Metadata, ToolCallId, ToolObservationV2, ToolOutcomeStatus, ToolResult, ToolRisk,
    };

    fn prepared(
        objective: &str,
        epoch: u64,
        tool_requirement: PromptToolRequirement,
    ) -> PreparedTaskState {
        PreparedTaskState::from_persisted(
            objective.to_string(),
            epoch,
            epoch,
            PromptCompletionIntent {
                tool_requirement,
                ..PromptCompletionIntent::default()
            },
        )
    }

    fn tool(name: &str, risk: ToolRisk) -> ToolSpec {
        ToolSpec::builtin(name, "test", "test tool", risk, r#"{"type":"object"}"#)
    }

    #[test]
    fn focus_tracks_authoritative_unresolved_work() {
        let empty = AgentTaskContract::default();
        assert_eq!(
            AgentCognitiveState::project(
                &prepared("answer", 0, PromptToolRequirement::None),
                &empty,
            )
            .focus(),
            AgentCognitiveFocus::Answer
        );

        let mut action = AgentTaskContract::default();
        action.require_tool_success("shell.run");
        assert_eq!(
            AgentCognitiveState::project(
                &prepared("run tests", 0, PromptToolRequirement::Effects),
                &action,
            )
            .focus(),
            AgentCognitiveFocus::Act
        );

        let mut evidence = AgentTaskContract::default();
        evidence.replace_prompt_required_tool_successes(2, ["file.read"]);
        assert_eq!(
            AgentCognitiveState::project(
                &prepared("inspect", 2, PromptToolRequirement::ReadOnly),
                &evidence,
            )
            .focus(),
            AgentCognitiveFocus::GatherEvidence
        );

        let mut verification = AgentTaskContract::default();
        verification.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        verification.record_tool_outcome(
            "file.write",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        assert_eq!(
            AgentCognitiveState::project(
                &prepared("modify", 0, PromptToolRequirement::Effects),
                &verification,
            )
            .focus(),
            AgentCognitiveFocus::Verify
        );
    }

    #[test]
    fn blockers_choose_replan_or_report_without_changing_contract_state() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("shell.run");
        contract.require_tool_success("file.read");
        contract.begin_action_denial_epoch(3);
        contract.record_action_denial(
            "shell.run",
            &"a".repeat(64),
            &AgentActionDenialFeedback::user_permission(),
        );
        let before = contract.clone();
        let prepared = prepared("inspect safely", 3, PromptToolRequirement::Effects);
        let state = AgentCognitiveState::project(&prepared, &contract);

        assert_eq!(state.focus(), AgentCognitiveFocus::Replan);
        assert_eq!(contract, before, "projection must be side-effect free");

        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert_eq!(
            AgentCognitiveState::project(&prepared, &contract).focus(),
            AgentCognitiveFocus::ReportBlocked
        );
    }

    #[test]
    fn renewed_epoch_excludes_old_prompt_evidence() {
        let mut contract = AgentTaskContract::default();
        contract.replace_prompt_required_tool_successes(1, ["file.read"]);
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"old-epoch-secret.txt"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        contract.replace_prompt_required_tool_successes(2, ["file.read"]);

        let state = AgentCognitiveState::project(
            &prepared("inspect the new epoch", 2, PromptToolRequirement::ReadOnly),
            &contract,
        );
        let encoded = state.to_bounded_json().expect("bounded state");

        assert_eq!(state.focus(), AgentCognitiveFocus::GatherEvidence);
        assert!(!encoded.contains("old-epoch-secret"));
        assert!(serde_json::from_str::<serde_json::Value>(&encoded)
            .unwrap()
            .get("evidence")
            .is_none());
    }

    #[test]
    fn projection_is_bounded_and_contains_no_objective_input_or_reasoning_text() {
        let raw_objective = "inspect private-objective-sentinel";
        let raw_input = r#"{"path":"private-input-sentinel.txt"}"#;
        let mut contract = AgentTaskContract::default();
        for index in 0..24 {
            let tool = format!("reader.{index:02}.{}", "x".repeat(96));
            contract.require_tool_success(tool.clone());
            contract.record_tool_outcome(
                &tool,
                raw_input,
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
            );
        }

        let state = AgentCognitiveState::project(
            &prepared(raw_objective, 0, PromptToolRequirement::ReadOnly),
            &contract,
        );
        let encoded = state.to_bounded_json().expect("bounded state");

        assert!(encoded.len() <= COGNITIVE_STATE_MAX_BYTES);
        assert!(encoded.contains(COGNITIVE_STATE_SCHEMA));
        assert!(!encoded.contains(raw_objective));
        assert!(!encoded.contains("private-input-sentinel"));
        assert!(!encoded.contains("reasoning"));
        assert_eq!(state.progress_fingerprint().unwrap().len(), 64);
    }

    #[test]
    fn progress_fingerprint_is_stable_and_changes_with_authoritative_progress() {
        let prepared = prepared("inspect", 0, PromptToolRequirement::ReadOnly);
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("file.read");
        let before = AgentCognitiveState::project(&prepared, &contract);
        assert_eq!(
            before.progress_fingerprint(),
            AgentCognitiveState::project(&prepared, &contract).progress_fingerprint()
        );

        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        let after = AgentCognitiveState::project(&prepared, &contract);

        assert_ne!(before.progress_fingerprint(), after.progress_fingerprint());
        assert_eq!(after.focus(), AgentCognitiveFocus::Answer);
    }

    #[test]
    fn available_required_tool_is_exposed_as_a_bounded_action_hint() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("file.read");
        let state = AgentCognitiveState::project_with_tools(
            &prepared("inspect", 0, PromptToolRequirement::ReadOnly),
            &contract,
            &[tool("file.read", ToolRisk::ReadOnly)],
        );

        assert_eq!(state.actions.len(), 1);
        assert_eq!(state.actions[0].kind, CognitiveActionKind::Act);
        assert_eq!(state.actions[0].one_of, ["file.read"]);
    }

    #[test]
    fn denied_or_unavailable_required_tools_are_not_suggested() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("shell.run");
        contract.require_tool_success("missing.tool");
        contract.begin_action_denial_epoch(0);
        contract.record_action_denial(
            "shell.run",
            &"b".repeat(64),
            &AgentActionDenialFeedback::user_permission(),
        );

        let state = AgentCognitiveState::project_with_tools(
            &prepared("act", 0, PromptToolRequirement::Effects),
            &contract,
            &[
                tool("shell.run", ToolRisk::ExecutesProcess),
                tool("file.read", ToolRisk::ReadOnly),
            ],
        );

        assert!(state.actions.is_empty());
    }

    #[test]
    fn compact_cognitive_state_and_adaptive_loop_scaling_contract() {
        const REQUIREMENT_COUNT: usize = 2_048;

        let mut contract = AgentTaskContract::default();
        let mut tools = Vec::with_capacity(REQUIREMENT_COUNT);
        for index in 0..REQUIREMENT_COUNT {
            let name = format!("scale.tool.{index:04}");
            contract.require_tool_success(name.clone());
            tools.push(tool(&name, ToolRisk::ReadOnly));
        }
        let state = AgentCognitiveState::project_with_tools(
            &prepared("bounded scaling", 0, PromptToolRequirement::ReadOnly),
            &contract,
            &tools,
        );
        let cognitive_json = state.to_bounded_json().expect("bounded cognitive state");

        assert!(cognitive_json.len() <= COGNITIVE_STATE_MAX_BYTES);
        assert!(state.actions.len() <= MAX_COGNITIVE_ACTIONS);
        assert!(state
            .actions
            .iter()
            .all(|action| action.one_of.len() <= MAX_COGNITIVE_ACTION_TOOLS));
        assert!(state
            .truncation
            .as_ref()
            .is_some_and(|truncation| truncation.actions > 0));

        let result = ToolResult {
            invocation_id: ToolCallId("scaling-call".to_string()),
            status: ToolOutcomeStatus::Succeeded,
            output: "excluded output".to_string(),
            content: Vec::new(),
            structured_output_json: None,
            artifacts: Vec::new(),
            failure: None,
            model_observation: Some(ToolObservationV2::new(
                "scale.tool",
                "excluded summary",
                "excluded evidence",
                true,
                [("revision".to_string(), "1".to_string())]
                    .into_iter()
                    .collect(),
            )),
            metadata: Metadata::new(),
        };
        let mut cursor = AdaptiveLoopCursor::for_steer_epoch(0);
        for index in 0..REQUIREMENT_COUNT {
            cursor.observe_tool_result(
                0,
                "scale.tool",
                &format!("sha256:scale:{}", index % MAX_RECENT_SEMANTIC_ACTIONS),
                &result,
                Some(&agent_core::ToolEffectSemantics::ReadOnly),
                None,
            );
        }
        let cursor_json = serde_json::to_vec(&cursor).expect("bounded adaptive cursor");

        assert_eq!(cursor.no_gain_count(), MAX_ADAPTIVE_NO_GAIN_COUNT);
        assert!(cursor.replan_emitted());
        assert_eq!(cursor.semantic_history_len(), MAX_RECENT_SEMANTIC_ACTIONS);
        assert!(cursor_json.len() <= 4_096);
        println!(
            "cindx.agent-cognitive-loop-scaling.v1 requirements={REQUIREMENT_COUNT} cognitive_bytes={} actions={} cursor_bytes={}",
            cognitive_json.len(),
            state.actions.len(),
            cursor_json.len()
        );
    }
}
