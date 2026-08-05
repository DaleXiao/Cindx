use super::{fingerprint, AgentTaskContract, MAX_CONTEXT_TARGETS};
use crate::InteractionSurface;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const GOAL_DELTA_SCHEMA: &str = "cindx.agent.goal-delta.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentGoalDeltaKind {
    ObligationSatisfied,
    GroundingAcquired,
    WorkspaceVerified,
    InteractionVerified,
}

impl AgentGoalDeltaKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::ObligationSatisfied => "obligation_satisfied",
            Self::GroundingAcquired => "grounding_acquired",
            Self::WorkspaceVerified => "workspace_verified",
            Self::InteractionVerified => "interaction_verified",
        }
    }
}

/// A bounded receipt for new state that advances the active task contract.
///
/// Raw tool input and output never enter this receipt. Its identity is derived
/// only from obligation ids, prompt scope, and stable verified postconditions,
/// so changing arguments or wording cannot manufacture budget credit without
/// advancing the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentGoalDelta {
    schema: String,
    kinds: Vec<AgentGoalDeltaKind>,
    receipt_ids: Vec<u64>,
    fingerprint: String,
}

impl AgentGoalDelta {
    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn kinds(&self) -> &[AgentGoalDeltaKind] {
        &self.kinds
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn receipt_ids(&self) -> &[u64] {
        &self.receipt_ids
    }

    pub fn kind_labels(&self) -> String {
        self.kinds
            .iter()
            .map(|kind| kind.label())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoalProgressState {
    satisfied_obligations: BTreeSet<u64>,
    grounding_receipts: BTreeSet<u64>,
    verified_workspace_receipt: Option<u64>,
    pending_interactions: BTreeMap<InteractionSurface, String>,
    prompt_scope: (u64, u64),
}

impl AgentTaskContract {
    pub(crate) fn goal_progress_state(&self) -> GoalProgressState {
        let mut satisfied_obligations = BTreeSet::new();
        for tool in self
            .required_tool_successes
            .intersection(&self.successful_tools)
        {
            satisfied_obligations.insert(receipt_id(&("required_tool", "run", tool)));
        }
        for tool in self
            .prompt_required_tool_successes
            .intersection(&self.prompt_successful_tools)
        {
            satisfied_obligations.insert(receipt_id(&(
                "required_tool",
                "steer",
                self.prompt_requirement_epoch,
                tool,
            )));
        }
        for (requirement, alternatives) in &self.required_any_tool_successes {
            if !alternatives.is_disjoint(&self.successful_tools) {
                satisfied_obligations.insert(receipt_id(&("any_tool", "run", requirement)));
            }
        }
        for (requirement, alternatives) in &self.prompt_required_any_tool_successes {
            if !alternatives.is_disjoint(&self.prompt_successful_tools) {
                satisfied_obligations.insert(receipt_id(&(
                    "any_tool",
                    "steer",
                    self.prompt_requirement_epoch,
                    requirement,
                )));
            }
        }

        let grounding_receipts = self
            .prompt_evidence_requirements
            .iter()
            .filter_map(|(requirement, state)| {
                state.receipt.as_ref().map(|_| {
                    receipt_id(&(
                        "grounding",
                        "steer",
                        self.prompt_evidence_epoch,
                        requirement,
                    ))
                })
            })
            .collect();

        let prompt_scope = (self.prompt_requirement_epoch, self.prompt_evidence_epoch);
        let verified_workspace_receipt = (self.mutation_epoch > 0
            && self.verified_mutation_epoch >= self.mutation_epoch)
            .then(|| {
                let bounded_targets = self
                    .mutation_targets
                    .iter()
                    .take(MAX_CONTEXT_TARGETS)
                    .collect::<Vec<_>>();
                receipt_id(&("workspace_verified", prompt_scope, bounded_targets))
            });

        GoalProgressState {
            satisfied_obligations,
            grounding_receipts,
            verified_workspace_receipt,
            pending_interactions: self.pending_interactions.clone(),
            prompt_scope,
        }
    }

    pub(crate) fn goal_delta_since(&self, previous: &GoalProgressState) -> Option<AgentGoalDelta> {
        let current = self.goal_progress_state();
        let new_obligations = current
            .satisfied_obligations
            .difference(&previous.satisfied_obligations)
            .cloned()
            .collect::<Vec<_>>();
        let new_grounding = current
            .grounding_receipts
            .difference(&previous.grounding_receipts)
            .cloned()
            .collect::<Vec<_>>();
        let workspace_verified = current
            .verified_workspace_receipt
            .as_ref()
            .filter(|receipt| previous.verified_workspace_receipt.as_ref() != Some(*receipt))
            .cloned();
        let verified_interactions = previous
            .pending_interactions
            .iter()
            .filter(|(surface, action)| {
                current.pending_interactions.get(*surface) != Some(*action)
                    && !current.pending_interactions.contains_key(*surface)
            })
            .map(|(surface, action)| {
                receipt_id(&(
                    "interaction_verified",
                    current.prompt_scope,
                    surface.label(),
                    action,
                ))
            })
            .collect::<Vec<_>>();

        let mut kinds = BTreeSet::new();
        if !new_obligations.is_empty() {
            kinds.insert(AgentGoalDeltaKind::ObligationSatisfied);
        }
        if !new_grounding.is_empty() {
            kinds.insert(AgentGoalDeltaKind::GroundingAcquired);
        }
        if workspace_verified.is_some() {
            kinds.insert(AgentGoalDeltaKind::WorkspaceVerified);
        }
        if !verified_interactions.is_empty() {
            kinds.insert(AgentGoalDeltaKind::InteractionVerified);
        }
        if kinds.is_empty() {
            return None;
        }

        let mut receipt_ids = Vec::with_capacity(4);
        if !new_obligations.is_empty() {
            receipt_ids.push(receipt_id(&(
                "obligation_frontier",
                &current.satisfied_obligations,
            )));
        }
        if !new_grounding.is_empty() {
            receipt_ids.push(receipt_id(&(
                "grounding_frontier",
                &current.grounding_receipts,
            )));
        }
        receipt_ids.extend(workspace_verified);
        receipt_ids.extend(verified_interactions);
        receipt_ids.sort_unstable();
        receipt_ids.dedup();

        let identity = serde_json::to_string(&(GOAL_DELTA_SCHEMA, &receipt_ids))
            .expect("bounded goal delta identity should serialize");
        Some(AgentGoalDelta {
            schema: GOAL_DELTA_SCHEMA.to_string(),
            kinds: kinds.into_iter().collect(),
            receipt_ids,
            fingerprint: fingerprint(&identity),
        })
    }
}

fn receipt_id(value: &impl Serialize) -> u64 {
    let encoded = serde_json::to_vec(value).expect("goal delta receipt identity should serialize");
    let digest = Sha256::digest(encoded);
    u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("sha256 prefix should contain eight bytes"),
    )
}

#[cfg(test)]
impl AgentGoalDelta {
    pub(crate) fn synthetic_for_test(fingerprint: &str) -> Self {
        Self {
            schema: GOAL_DELTA_SCHEMA.to_string(),
            kinds: vec![AgentGoalDeltaKind::ObligationSatisfied],
            receipt_ids: vec![receipt_id(&("synthetic", fingerprint))],
            fingerprint: fingerprint.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        start_agent_loop, AgentKernel, AgentRunControl, AgentRuntimeConfig, AgentToolRequest,
        RunBudget, RunStopReason, WorkspaceVerificationPolicy,
    };
    use agent_core::{TaskId, ToolCallId, ToolOutcomeStatus, ToolRisk, ToolSpec};
    use std::time::Duration;

    fn tool(name: &str, risk: ToolRisk) -> ToolSpec {
        ToolSpec::builtin(name, "test", name, risk, r#"{"type":"object"}"#)
    }

    fn request(id: &str, tool_name: &str, input: &str) -> AgentToolRequest {
        AgentToolRequest {
            call_id: ToolCallId(id.to_string()),
            tool_name: tool_name.to_string(),
            input: input.to_string(),
        }
    }

    fn test_budget() -> RunBudget {
        RunBudget {
            max_duration: Duration::from_secs(5),
            model_call_timeout: Duration::from_secs(2),
            tool_call_timeout: Duration::from_secs(2),
            initial_model_calls: 1,
            max_model_calls: 3,
            model_calls_per_extension: 1,
            initial_tool_calls: 2,
            max_tool_calls: 4,
            tool_calls_per_extension: 1,
            no_progress_timeout: Duration::from_secs(1),
            max_identical_actions: 3,
            initial_agent_turns: 1,
            max_agent_turns: 3,
            agent_turns_per_extension: 1,
            max_repair_attempts: 1,
            terminal_model_call_reserve: 0,
            terminal_time_reserve: Duration::ZERO,
            max_total_tokens: 1_000,
            max_physical_model_attempts: 8,
            terminal_token_reserve: 0,
            terminal_physical_model_attempt_reserve: 0,
        }
    }

    #[test]
    fn agent_goal_progress_contract() {
        let tools = vec![
            tool("file.read", ToolRisk::ReadOnly),
            tool("file.write", ToolRisk::WritesWorkspace),
            tool("process.run", ToolRisk::ExecutesProcess),
            tool("browser.click", ToolRisk::UsesNetwork),
            tool("browser.capture", ToolRisk::UsesNetwork),
        ];
        let mut runtime = start_agent_loop(
            TaskId("goal-delta".to_string()),
            "complete the task",
            AgentRuntimeConfig::default(),
        );

        let unrelated = AgentKernel::new(&mut runtime, &tools).apply_tool_observation(
            &request("run-unrelated", "process.run", r#"{"command":"pwd"}"#),
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ExecutesProcess),
            "unrelated discovery",
        );
        assert_eq!(unrelated, None, "ordinary success is not goal progress");

        runtime.task_contract.require_tool_success("file.read");
        for status in [
            ToolOutcomeStatus::Failed,
            ToolOutcomeStatus::Denied,
            ToolOutcomeStatus::Cancelled,
        ] {
            assert_eq!(
                AgentKernel::new(&mut runtime, &tools).apply_tool_observation(
                    &request("read-failed", "file.read", r#"{"path":"goal.md"}"#),
                    &status,
                    Some(&ToolRisk::ReadOnly),
                    "no result",
                ),
                None
            );
        }
        let obligation = AgentKernel::new(&mut runtime, &tools)
            .apply_tool_observation(
                &request("read-goal", "file.read", r#"{"path":"goal.md"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "goal evidence",
            )
            .expect("the required tool should satisfy one obligation");
        assert_eq!(
            obligation.kinds(),
            &[AgentGoalDeltaKind::ObligationSatisfied]
        );
        assert_eq!(obligation.schema(), GOAL_DELTA_SCHEMA);
        assert!(serde_json::to_vec(&obligation).unwrap().len() < 512);
        assert_eq!(
            AgentKernel::new(&mut runtime, &tools).apply_tool_observation(
                &request("read-repeat", "file.read", r#"{"path":"other.md"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "different output",
            ),
            None,
            "a satisfied obligation cannot be replayed for credit"
        );

        let control = AgentRunControl::with_budget(test_budget());
        assert_eq!(control.begin_model_call("first"), Ok(1));
        assert!(control.record_goal_delta_at(0, &obligation));
        assert_eq!(control.begin_model_call("second"), Ok(2));
        assert!(!control.record_goal_delta_at(0, &obligation));
        assert_eq!(
            control.begin_model_call("third"),
            Err(RunStopReason::ModelCallBudgetExceeded),
            "one delta may extend a budget lane only once"
        );
        let generic_checkpoint = AgentRunControl::with_budget(test_budget());
        assert_eq!(generic_checkpoint.begin_model_call("first"), Ok(1));
        assert!(generic_checkpoint.record_checkpoint("diagnostic", "tool", "ordinary-success"));
        assert_eq!(
            generic_checkpoint.begin_model_call("second"),
            Err(RunStopReason::ModelCallBudgetExceeded),
            "generic checkpoints must not unlock budget"
        );

        let snapshot_source = AgentRunControl::with_budget(test_budget());
        assert!(snapshot_source.record_goal_delta_at(0, &obligation));
        let snapshot_resumed = AgentRunControl::from_snapshot(snapshot_source.snapshot());
        assert!(!snapshot_resumed.record_goal_delta_at(0, &obligation));
        assert_eq!(snapshot_resumed.begin_model_call("first"), Ok(1));
        assert_eq!(snapshot_resumed.begin_model_call("extended"), Ok(2));
        let stale_control = AgentRunControl::with_budget(test_budget());
        assert_eq!(stale_control.request_steer("new-objective"), Ok(true));
        assert!(!stale_control.record_goal_delta_at(0, &obligation));
        assert_eq!(stale_control.progress().checkpoints, 0);

        let mut grounding = start_agent_loop(
            TaskId("grounding".to_string()),
            "inspect goal.md",
            AgentRuntimeConfig::default(),
        );
        grounding.task_contract.replace_prompt_evidence_requirement(
            4,
            Some("workspace"),
            ["file.read"],
        );
        let grounded = AgentKernel::new(&mut grounding, &tools)
            .apply_tool_observation(
                &request("ground", "file.read", r#"{"path":"goal.md"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "substantive workspace evidence",
            )
            .expect("first substantive grounding should advance the contract");
        assert_eq!(grounded.kinds(), &[AgentGoalDeltaKind::GroundingAcquired]);
        assert_eq!(
            AgentKernel::new(&mut grounding, &tools).apply_tool_observation(
                &request("ground-repeat", "file.read", r#"{"path":"other.md"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "new wording",
            ),
            None
        );

        let mut mutation = start_agent_loop(
            TaskId("mutation".to_string()),
            "change and verify",
            AgentRuntimeConfig::default(),
        );
        mutation.task_contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        assert_eq!(
            AgentKernel::new(&mut mutation, &tools).apply_tool_observation(
                &request("write", "file.write", r#"{"path":"src/lib.rs"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::WritesWorkspace),
                "written",
            ),
            None,
            "an unverified mutation is not budget credit"
        );
        assert_eq!(
            AgentKernel::new(&mut mutation, &tools).apply_tool_observation(
                &request("read-miss", "file.read", r#"{"path":"README.md"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "unrelated read",
            ),
            None
        );
        let verified = AgentKernel::new(&mut mutation, &tools)
            .apply_tool_observation(
                &request("verify", "process.run", r#"{"command":"cargo test"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ExecutesProcess),
                "tests passed",
            )
            .expect("matching verification should advance the postcondition");
        assert_eq!(verified.kinds(), &[AgentGoalDeltaKind::WorkspaceVerified]);
        let mutation_control = AgentRunControl::with_budget(test_budget());
        assert!(mutation_control.record_goal_delta_at(0, &verified));
        assert_eq!(
            AgentKernel::new(&mut mutation, &tools).apply_tool_observation(
                &request("write-repeat", "file.write", r#"{"path":"src/lib.rs"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::WritesWorkspace),
                "written again",
            ),
            None
        );
        let repeated_verification = AgentKernel::new(&mut mutation, &tools)
            .apply_tool_observation(
                &request(
                    "verify-repeat",
                    "process.run",
                    r#"{"command":"cargo test"}"#,
                ),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ExecutesProcess),
                "tests passed again",
            )
            .expect("the repeated pair still closes its current postcondition");
        assert_eq!(repeated_verification.fingerprint(), verified.fingerprint());
        assert!(!mutation_control.record_goal_delta_at(0, &repeated_verification));

        let mut combined = start_agent_loop(
            TaskId("combined-delta".to_string()),
            "change, inspect, and verify",
            AgentRuntimeConfig::default(),
        );
        combined.task_contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        combined.task_contract.require_tool_success("file.read");
        AgentKernel::new(&mut combined, &tools).apply_tool_observation(
            &request("combined-write-1", "file.write", r#"{"path":"src/lib.rs"}"#),
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
            "written",
        );
        let workspace_only = AgentKernel::new(&mut combined, &tools)
            .apply_tool_observation(
                &request(
                    "combined-verify-1",
                    "process.run",
                    r#"{"command":"cargo test"}"#,
                ),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ExecutesProcess),
                "tests passed",
            )
            .expect("the first verification should close the workspace postcondition");
        let combined_control = AgentRunControl::with_budget(test_budget());
        assert!(combined_control.record_goal_delta_at(0, &workspace_only));

        AgentKernel::new(&mut combined, &tools).apply_tool_observation(
            &request("combined-write-2", "file.write", r#"{"path":"src/lib.rs"}"#),
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
            "written again",
        );
        let obligation_and_workspace = AgentKernel::new(&mut combined, &tools)
            .apply_tool_observation(
                &request("combined-verify-2", "file.read", r#"{"path":"src/lib.rs"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "verified content",
            )
            .expect("the same transition should retain both new constituents");
        assert_eq!(
            obligation_and_workspace.kinds(),
            &[
                AgentGoalDeltaKind::ObligationSatisfied,
                AgentGoalDeltaKind::WorkspaceVerified,
            ]
        );
        assert!(serde_json::to_vec(&obligation_and_workspace).unwrap().len() < 512);
        assert!(
            combined_control.record_goal_delta_at(0, &obligation_and_workspace),
            "the new obligation constituent must not be hidden by the repeated workspace receipt"
        );

        AgentKernel::new(&mut combined, &tools).apply_tool_observation(
            &request("combined-write-3", "file.write", r#"{"path":"src/lib.rs"}"#),
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
            "written once more",
        );
        let repeated_workspace = AgentKernel::new(&mut combined, &tools)
            .apply_tool_observation(
                &request("combined-verify-3", "file.read", r#"{"path":"src/lib.rs"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "verified again",
            )
            .expect("the repeated pair still closes its current postcondition");
        assert_eq!(
            repeated_workspace.fingerprint(),
            workspace_only.fingerprint()
        );
        assert!(!combined_control.record_goal_delta_at(0, &repeated_workspace));

        let mut interaction = start_agent_loop(
            TaskId("interaction".to_string()),
            "click and observe",
            AgentRuntimeConfig::default(),
        );
        assert_eq!(
            AgentKernel::new(&mut interaction, &tools).apply_tool_observation(
                &request("click", "browser.click", r#"{"target":"submit"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::UsesNetwork),
                "clicked",
            ),
            None,
            "an unverified interaction is not budget credit"
        );
        let interaction_verified = AgentKernel::new(&mut interaction, &tools)
            .apply_tool_observation(
                &request("capture", "browser.capture", "{}"),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::UsesNetwork),
                "captured changed state",
            )
            .expect("the matching observation should verify the interaction");
        assert_eq!(
            interaction_verified.kinds(),
            &[AgentGoalDeltaKind::InteractionVerified]
        );
        let interaction_control = AgentRunControl::with_budget(test_budget());
        assert!(interaction_control.record_goal_delta_at(0, &interaction_verified));
        assert_eq!(
            AgentKernel::new(&mut interaction, &tools).apply_tool_observation(
                &request("click-repeat", "browser.click", r#"{"target":"submit"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::UsesNetwork),
                "clicked again",
            ),
            None
        );
        let repeated_interaction = AgentKernel::new(&mut interaction, &tools)
            .apply_tool_observation(
                &request("capture-pair-repeat", "browser.capture", "{}"),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::UsesNetwork),
                "captured changed state again",
            )
            .expect("the repeated pair still closes its current postcondition");
        assert_eq!(
            repeated_interaction.fingerprint(),
            interaction_verified.fingerprint()
        );
        assert!(!interaction_control.record_goal_delta_at(0, &repeated_interaction));
        assert_eq!(
            AgentKernel::new(&mut interaction, &tools).apply_tool_observation(
                &request("capture-repeat", "browser.capture", "{}"),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::UsesNetwork),
                "captured again",
            ),
            None
        );

        let mut large_grounding = start_agent_loop(
            TaskId("large-grounding".to_string()),
            "inspect goal.md",
            AgentRuntimeConfig::default(),
        );
        large_grounding
            .task_contract
            .replace_prompt_evidence_requirement(4, Some("workspace"), ["file.read"]);
        let large = AgentKernel::new(&mut large_grounding, &tools)
            .apply_tool_observation(
                &request("ground", "file.read", r#"{"path":"goal.md"}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                &"x".repeat(100_000),
            )
            .expect("large grounding should produce the same bounded identity");
        assert_eq!(large.fingerprint(), grounded.fingerprint());
        assert!(serde_json::to_vec(&large).unwrap().len() < 512);

        println!("{GOAL_DELTA_SCHEMA}");
    }
}
