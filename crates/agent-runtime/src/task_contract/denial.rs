use super::{AgentTaskContract, ContractEvidence, ContractEvidenceKind, MAX_CONTRACT_EVIDENCE};
use agent_core::ToolRisk;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ACTION_DENIAL_SCHEMA: &str = "cindx.agent.action-denial.v1";

const MAX_ACTION_DENIALS: usize = 8;
const MAX_DENIAL_CODE_BYTES: usize = 96;
const MAX_DENIAL_TOOL_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActionDenialKind {
    UserPermission,
    RuntimePolicy,
    CapabilityUnavailable,
    RepeatedAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActionDenialScope {
    ExactAction,
    Capability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActionRecovery {
    Replan,
    FinalizeBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActionDenialFeedback {
    pub kind: AgentActionDenialKind,
    pub code: &'static str,
    pub scope: AgentActionDenialScope,
    pub recovery: AgentActionRecovery,
}

impl AgentActionDenialFeedback {
    pub fn user_permission() -> Self {
        Self {
            kind: AgentActionDenialKind::UserPermission,
            code: "user_permission_denied",
            scope: AgentActionDenialScope::ExactAction,
            recovery: AgentActionRecovery::FinalizeBlocked,
        }
    }

    pub fn runtime_policy(code: &'static str, recovery: AgentActionRecovery) -> Self {
        Self {
            kind: AgentActionDenialKind::RuntimePolicy,
            code,
            scope: AgentActionDenialScope::ExactAction,
            recovery,
        }
    }

    pub fn capability_unavailable(code: &'static str) -> Self {
        Self {
            kind: AgentActionDenialKind::CapabilityUnavailable,
            code,
            scope: AgentActionDenialScope::Capability,
            recovery: AgentActionRecovery::Replan,
        }
    }

    pub fn repeated_action(code: &'static str) -> Self {
        Self {
            kind: AgentActionDenialKind::RepeatedAction,
            code,
            scope: AgentActionDenialScope::ExactAction,
            recovery: AgentActionRecovery::FinalizeBlocked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentActionDenial {
    pub schema: String,
    pub contract_epoch: u64,
    pub evidence_sequence: u64,
    pub tool_name: String,
    pub input_fingerprint: String,
    pub kind: AgentActionDenialKind,
    pub code: String,
    pub scope: AgentActionDenialScope,
    pub recovery: AgentActionRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskContractDenialSummary {
    schema: &'static str,
    contract_epoch: u64,
    tool_name: String,
    kind: AgentActionDenialKind,
    code: String,
    recovery: AgentActionRecovery,
}

impl AgentTaskContract {
    pub fn begin_action_denial_epoch(&mut self, contract_epoch: u64) {
        if self.action_denial_epoch == contract_epoch {
            return;
        }
        self.action_denial_epoch = contract_epoch;
        self.action_denials.clear();
        self.action_denial_replan_used = false;
        self.gate_attempts
            .retain(|key, _| !key.starts_with("action_denial:"));
    }

    pub fn record_action_denial(
        &mut self,
        tool_name: &str,
        input_fingerprint: &str,
        feedback: &AgentActionDenialFeedback,
    ) -> Option<AgentActionDenial> {
        let tool_name = tool_name.trim();
        let input_fingerprint = input_fingerprint.trim().to_ascii_lowercase();
        if tool_name.is_empty()
            || tool_name.len() > MAX_DENIAL_TOOL_BYTES
            || !is_sha256_hex(&input_fingerprint)
            || !stable_denial_code_is_valid(feedback.code)
        {
            return None;
        }
        if let Some(existing) = self.action_denials.iter().find(|denial| {
            denial.contract_epoch == self.action_denial_epoch
                && denial.tool_name == tool_name
                && denial.input_fingerprint == input_fingerprint
                && denial.code == feedback.code
        }) {
            return Some(existing.clone());
        }

        let recovery = match feedback.recovery {
            AgentActionRecovery::FinalizeBlocked => AgentActionRecovery::FinalizeBlocked,
            AgentActionRecovery::Replan if !self.action_denial_replan_used => {
                self.action_denial_replan_used = true;
                AgentActionRecovery::Replan
            }
            AgentActionRecovery::Replan => AgentActionRecovery::FinalizeBlocked,
        };
        self.next_sequence = self.next_sequence.saturating_add(1);
        let evidence_sequence = self.next_sequence;
        self.evidence.push(ContractEvidence {
            sequence: evidence_sequence,
            kind: ContractEvidenceKind::Denial,
            source: tool_name.to_string(),
            input_fingerprint: input_fingerprint.clone(),
        });
        let denial = AgentActionDenial {
            schema: ACTION_DENIAL_SCHEMA.to_string(),
            contract_epoch: self.action_denial_epoch,
            evidence_sequence,
            tool_name: tool_name.to_string(),
            input_fingerprint,
            kind: feedback.kind,
            code: feedback.code.to_string(),
            scope: feedback.scope,
            recovery,
        };
        self.action_denials.push(denial.clone());
        if self.action_denials.len() > MAX_ACTION_DENIALS {
            let excess = self.action_denials.len() - MAX_ACTION_DENIALS;
            self.action_denials.drain(..excess);
        }
        self.trim_contract_evidence();
        Some(denial)
    }

    pub fn action_denial_for_invocation(
        &self,
        tool_name: &str,
        input_fingerprint: &str,
        risk: Option<&ToolRisk>,
    ) -> Option<AgentActionDenialFeedback> {
        let input_fingerprint = input_fingerprint.trim();
        if self.action_denials.iter().any(|denial| {
            denial.contract_epoch == self.action_denial_epoch
                && denial.tool_name == tool_name
                && match denial.scope {
                    AgentActionDenialScope::ExactAction => {
                        denial.input_fingerprint == input_fingerprint
                    }
                    AgentActionDenialScope::Capability => true,
                }
        }) {
            return Some(AgentActionDenialFeedback::repeated_action(
                "previously_denied_action",
            ));
        }
        let finalization_only = self.action_denials.iter().any(|denial| {
            denial.contract_epoch == self.action_denial_epoch
                && denial.recovery == AgentActionRecovery::FinalizeBlocked
        });
        (finalization_only && !matches!(risk, Some(ToolRisk::ReadOnly)))
            .then(|| AgentActionDenialFeedback::repeated_action("denial_replan_exhausted"))
    }

    pub fn has_user_permission_finalization(
        &self,
        tool_name: &str,
        input_fingerprint: &str,
    ) -> bool {
        self.action_denials.iter().any(|denial| {
            denial.contract_epoch == self.action_denial_epoch
                && denial.tool_name == tool_name
                && denial.input_fingerprint == input_fingerprint
                && denial.kind == AgentActionDenialKind::UserPermission
                && denial.code == "user_permission_denied"
                && denial.scope == AgentActionDenialScope::ExactAction
                && denial.recovery == AgentActionRecovery::FinalizeBlocked
                && self.evidence.iter().any(|evidence| {
                    evidence.sequence == denial.evidence_sequence
                        && evidence.kind == ContractEvidenceKind::Denial
                        && evidence.source == denial.tool_name
                        && evidence.input_fingerprint == denial.input_fingerprint
                })
        })
    }

    pub(super) fn action_denial_for_tools<'a>(
        &'a self,
        tools: impl IntoIterator<Item = &'a str>,
    ) -> Option<&'a AgentActionDenial> {
        let tools = tools.into_iter().collect::<BTreeSet<_>>();
        self.action_denials
            .iter()
            .filter(|denial| {
                denial.contract_epoch == self.action_denial_epoch
                    && denial.recovery == AgentActionRecovery::FinalizeBlocked
                    && tools.contains(denial.tool_name.as_str())
                    && self.evidence.iter().any(|evidence| {
                        evidence.sequence == denial.evidence_sequence
                            && evidence.kind == ContractEvidenceKind::Denial
                    })
            })
            .max_by_key(|denial| (denial_priority(denial.kind), denial.evidence_sequence))
    }

    pub(super) fn action_denial_summaries(&self) -> Vec<TaskContractDenialSummary> {
        self.action_denials
            .iter()
            .filter(|denial| denial.contract_epoch == self.action_denial_epoch)
            .map(|denial| TaskContractDenialSummary {
                schema: ACTION_DENIAL_SCHEMA,
                contract_epoch: denial.contract_epoch,
                tool_name: denial.tool_name.clone(),
                kind: denial.kind,
                code: denial.code.clone(),
                recovery: denial.recovery,
            })
            .collect()
    }

    pub(super) fn validate_action_denials(&self) -> Result<(), &'static str> {
        if self.action_denials.len() > MAX_ACTION_DENIALS {
            return Err("too many action denials");
        }
        let mut identities = BTreeSet::new();
        for denial in &self.action_denials {
            if denial.schema != ACTION_DENIAL_SCHEMA
                || denial.contract_epoch != self.action_denial_epoch
                || denial.tool_name.trim().is_empty()
                || denial.tool_name.len() > MAX_DENIAL_TOOL_BYTES
                || !is_sha256_hex(&denial.input_fingerprint)
                || !stable_denial_code_is_valid(&denial.code)
                || !identities.insert((
                    denial.tool_name.as_str(),
                    denial.input_fingerprint.as_str(),
                    denial.code.as_str(),
                ))
                || !self.evidence.iter().any(|evidence| {
                    evidence.sequence == denial.evidence_sequence
                        && evidence.kind == ContractEvidenceKind::Denial
                        && evidence.source == denial.tool_name
                        && evidence.input_fingerprint == denial.input_fingerprint
                })
            {
                return Err("action denial state is invalid");
            }
        }
        if !self.action_denial_replan_used
            && self
                .action_denials
                .iter()
                .any(|denial| denial.recovery == AgentActionRecovery::Replan)
        {
            return Err("action denial replan state is inconsistent");
        }
        Ok(())
    }

    pub(super) fn trim_contract_evidence(&mut self) {
        if self.evidence.len() <= MAX_CONTRACT_EVIDENCE {
            return;
        }
        let mut pinned = self
            .action_denials
            .iter()
            .map(|denial| denial.evidence_sequence)
            .collect::<BTreeSet<_>>();
        pinned.extend(
            self.postcondition_bindings
                .values()
                .map(|binding| binding.action_sequence),
        );
        pinned.extend(
            self.postcondition_verification_receipts
                .iter()
                .flat_map(|receipt| [receipt.action_sequence, receipt.observation_sequence]),
        );
        let mut remaining = self.evidence.len() - MAX_CONTRACT_EVIDENCE;
        self.evidence.retain(|evidence| {
            if remaining > 0 && !pinned.contains(&evidence.sequence) {
                remaining -= 1;
                false
            } else {
                true
            }
        });
    }
}

fn denial_priority(kind: AgentActionDenialKind) -> u8 {
    match kind {
        AgentActionDenialKind::UserPermission => 4,
        AgentActionDenialKind::RuntimePolicy => 3,
        AgentActionDenialKind::CapabilityUnavailable => 2,
        AgentActionDenialKind::RepeatedAction => 1,
    }
}

pub(super) fn stable_denial_code_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DENIAL_CODE_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{tool_input_fingerprint, OutcomeSatisfaction, WorkspaceVerificationPolicy};
    use agent_core::{ToolOutcomeStatus, ToolSpec};

    fn tool(name: &str, risk: ToolRisk) -> ToolSpec {
        ToolSpec::builtin(name, "test", "test", risk, r#"{"type":"object"}"#)
    }

    #[test]
    fn permission_denial_is_blocked_not_satisfied_and_contains_no_raw_input() {
        let raw_input = r#"{"path":"private/denial-sentinel.txt"}"#;
        let fingerprint = tool_input_fingerprint("file.write", raw_input);
        let mut contract = AgentTaskContract::default();
        contract.begin_action_denial_epoch(4);
        contract.require_tool_success("file.write");

        let denial = contract
            .record_action_denial(
                "file.write",
                &fingerprint,
                &AgentActionDenialFeedback::user_permission(),
            )
            .expect("trusted denial should be recorded");
        let ledger = contract.outcome_ledger_shadow(4);

        assert_eq!(denial.recovery, AgentActionRecovery::FinalizeBlocked);
        assert_eq!(ledger.obligations.len(), 1);
        assert_eq!(
            ledger.obligations[0].satisfaction,
            OutcomeSatisfaction::Blocked
        );
        assert_eq!(
            ledger.obligations[0]
                .blocker
                .as_ref()
                .map(|blocker| blocker.code.as_str()),
            Some("user_permission_denied")
        );
        assert!(!contract.required_tool_satisfied("file.write"));
        let encoded = serde_json::to_string(&contract).expect("contract serializes");
        assert!(!encoded.contains("denial-sentinel"));
        assert_eq!(
            serde_json::from_str::<AgentTaskContract>(&encoded).expect("contract restores"),
            contract
        );
    }

    #[test]
    fn exact_denial_is_local_to_the_objective_and_does_not_block_safe_changes() {
        let first = tool_input_fingerprint("file.write", r#"{"path":"a.txt"}"#);
        let changed = tool_input_fingerprint("file.write", r#"{"path":"b.txt"}"#);
        let mut contract = AgentTaskContract::default();
        contract.begin_action_denial_epoch(7);
        contract.record_action_denial(
            "file.write",
            &first,
            &AgentActionDenialFeedback::user_permission(),
        );

        let blocked = contract
            .action_denial_for_invocation("file.write", &first, Some(&ToolRisk::WritesWorkspace))
            .expect("same action must be blocked before permission");
        assert_eq!(blocked.code, "previously_denied_action");
        assert!(contract
            .action_denial_for_invocation("file.read", &changed, Some(&ToolRisk::ReadOnly))
            .is_none());
        assert!(contract
            .action_denial_for_invocation("file.write", &changed, Some(&ToolRisk::WritesWorkspace))
            .is_some());

        contract.begin_action_denial_epoch(8);
        assert!(contract
            .action_denial_for_invocation("file.write", &first, Some(&ToolRisk::WritesWorkspace))
            .is_none());
    }

    #[test]
    fn policy_and_capability_denials_share_one_replan_token() {
        let mut contract = AgentTaskContract::default();
        contract.begin_action_denial_epoch(2);
        contract.require_tool_success("worker.write");
        contract.require_tool_success("missing.read");
        let first = contract
            .record_action_denial(
                "worker.write",
                &tool_input_fingerprint("worker.write", "{}"),
                &AgentActionDenialFeedback::runtime_policy(
                    "worker_tool_not_read_only",
                    AgentActionRecovery::Replan,
                ),
            )
            .expect("first denial records");
        let second = contract
            .record_action_denial(
                "missing.read",
                &tool_input_fingerprint("missing.read", "{}"),
                &AgentActionDenialFeedback::capability_unavailable("worker_tool_not_exposed"),
            )
            .expect("second denial records");

        assert_eq!(first.recovery, AgentActionRecovery::Replan);
        assert_eq!(second.recovery, AgentActionRecovery::FinalizeBlocked);
        let ledger = contract.outcome_ledger_shadow(2);
        assert_eq!(
            ledger
                .obligations
                .iter()
                .find(|obligation| obligation.blocker.is_none())
                .map(|obligation| obligation.satisfaction),
            Some(OutcomeSatisfaction::Pending),
            "the first denial must preserve the one bounded replan opportunity"
        );
        assert!(ledger.obligations.iter().any(|obligation| {
            obligation.satisfaction == OutcomeSatisfaction::Blocked
                && obligation
                    .blocker
                    .as_ref()
                    .map(|blocker| blocker.code.as_str())
                    == Some("worker_tool_not_exposed")
        }));
    }

    #[test]
    fn blocked_obligation_does_not_bypass_unrelated_pending_work() {
        let mut contract = AgentTaskContract::default();
        contract.begin_action_denial_epoch(1);
        contract.require_tool_success("file.write");
        contract.require_tool_success("file.read");
        contract.record_action_denial(
            "file.write",
            &tool_input_fingerprint("file.write", r#"{"path":"x"}"#),
            &AgentActionDenialFeedback::user_permission(),
        );
        let tools = vec![
            tool("file.write", ToolRisk::WritesWorkspace),
            tool("file.read", ToolRisk::ReadOnly),
        ];

        let instruction = contract
            .completion_instruction_for_task(&tools)
            .expect("gate evaluates")
            .expect("unrelated read remains pending");
        assert!(instruction.contains("file.read"));
        assert!(!instruction.contains("file.write"));

        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
        contract.merge_workspace_verification_policy(WorkspaceVerificationPolicy::NotRequired);
    }
}
