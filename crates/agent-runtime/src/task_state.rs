use crate::{
    normalized_failed_tool_signatures,
    task_state_lineage::{
        is_durable_message, is_transient_run_context, text_fingerprint, AgentTaskStateLineage,
        AgentTranscriptFingerprintAccumulator,
    },
    task_state_wire::PreparedTaskStateCheckpoint,
    AgentLoopState, AgentTaskContract, PreparedTaskState,
};
use agent_core::{Message, TaskId};
use std::{collections::BTreeMap, fmt};

pub const AGENT_TASK_STATE_SCHEMA: &str = "cindx.agent.task-state.v2";
pub const AGENT_TASK_STATE_SCHEMA_V1: &str = "cindx.agent.task-state.v1";
pub const MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTaskStateSnapshot {
    pub schema: String,
    pub task_id: String,
    pub user_prompt_fingerprint: String,
    pub transcript_fingerprint: String,
    pub durable_message_count: usize,
    pub turn: usize,
    pub max_turns: usize,
    pub failed_tool_signatures: BTreeMap<String, usize>,
    pub consecutive_empty_responses: usize,
    pub verification_gate_requests: usize,
    pub verified_interactions: usize,
    pub interaction_verification_gate_requests: usize,
    pub task_contract: AgentTaskContract,
    pub prepared_task_state: Option<PreparedTaskStateCheckpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTaskStateError {
    message: String,
}

impl AgentTaskStateError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AgentTaskStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentTaskStateError {}

impl AgentTaskStateSnapshot {
    pub fn capture(state: &AgentLoopState) -> Self {
        let transcript =
            AgentTranscriptFingerprintAccumulator::from_messages(state.messages.iter());
        let lineage = AgentTaskStateLineage::from_projection(&state.user_prompt, &transcript);
        Self::capture_with_lineage(state, lineage)
    }

    pub fn capture_with_lineage(state: &AgentLoopState, lineage: AgentTaskStateLineage) -> Self {
        Self::capture_with_lineage_and_prepared_task_state(
            state,
            lineage,
            state.prepared_task_state(),
        )
    }

    pub fn capture_with_lineage_and_prepared_task_state(
        state: &AgentLoopState,
        lineage: AgentTaskStateLineage,
        prepared_task_state: &PreparedTaskState,
    ) -> Self {
        Self {
            schema: AGENT_TASK_STATE_SCHEMA.to_string(),
            task_id: state.task_id.0.clone(),
            user_prompt_fingerprint: lineage.user_prompt_fingerprint,
            transcript_fingerprint: lineage.transcript_fingerprint,
            durable_message_count: lineage.durable_message_count,
            turn: state.turn,
            max_turns: state.max_turns,
            failed_tool_signatures: normalized_failed_tool_signatures(
                &state.failed_tool_signatures,
            ),
            consecutive_empty_responses: state.consecutive_empty_responses,
            verification_gate_requests: state.verification_gate_requests,
            verified_interactions: state.verified_interactions,
            interaction_verification_gate_requests: state.interaction_verification_gate_requests,
            task_contract: state.task_contract.clone(),
            prepared_task_state: Some(PreparedTaskStateCheckpoint::capture(prepared_task_state)),
        }
    }

    pub fn restore(
        &self,
        user_prompt: impl Into<String>,
        messages: Vec<Message>,
    ) -> Result<AgentLoopState, AgentTaskStateError> {
        let user_prompt = user_prompt.into();
        self.restore_with_effective_objective(user_prompt.clone(), messages, user_prompt)
    }

    pub fn restore_with_effective_objective(
        &self,
        user_prompt: impl Into<String>,
        messages: Vec<Message>,
        effective_objective: impl Into<String>,
    ) -> Result<AgentLoopState, AgentTaskStateError> {
        let user_prompt = user_prompt.into();
        let prepared_task_state = match self.prepared_task_state.as_ref() {
            Some(checkpoint) => checkpoint.restore_with_objective(effective_objective.into()),
            None => PreparedTaskState::initial(&user_prompt),
        };
        self.restore_with_prepared_task_state(user_prompt, messages, prepared_task_state)
    }

    pub fn restore_with_prepared_task_state(
        &self,
        user_prompt: impl Into<String>,
        messages: Vec<Message>,
        prepared_task_state: PreparedTaskState,
    ) -> Result<AgentLoopState, AgentTaskStateError> {
        self.validate()?;
        let user_prompt = user_prompt.into();
        if self.user_prompt_fingerprint != text_fingerprint(&user_prompt) {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint does not match the active user prompt",
            ));
        }
        let Some(messages) = self.matching_runtime_projection(messages) else {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint does not match the durable transcript",
            ));
        };
        if self
            .prepared_task_state
            .as_ref()
            .is_some_and(|checkpoint| !checkpoint.matches(&prepared_task_state))
        {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint does not match the prepared task state",
            ));
        }

        Ok(AgentLoopState {
            task_id: TaskId(self.task_id.clone()),
            user_prompt,
            messages,
            turn: self.turn,
            max_turns: self.max_turns,
            failed_tool_signatures: normalized_failed_tool_signatures(&self.failed_tool_signatures),
            consecutive_empty_responses: self.consecutive_empty_responses,
            verification_gate_requests: self.verification_gate_requests,
            verified_interactions: self.verified_interactions,
            interaction_verification_gate_requests: self.interaction_verification_gate_requests,
            task_contract: self.task_contract.clone(),
            adaptive_loop_cursor: crate::AdaptiveLoopCursor::for_steer_epoch(
                prepared_task_state.steer_epoch(),
            ),
            prepared_task_state,
            context_token_ledger: Default::default(),
            repetition_advisory: crate::RepetitionAdvisoryTracker::default(),
            loop_observers: crate::LoopObservers::default(),
            generation_temperature: None,
            reasoning_effort: None,
        })
    }

    fn matching_runtime_projection(&self, messages: Vec<Message>) -> Option<Vec<Message>> {
        let durable_indices = messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| is_durable_message(message).then_some(index))
            .collect::<Vec<_>>();
        if self.durable_message_count > durable_indices.len() {
            return None;
        }
        if self.durable_message_count == 0 {
            return (self.transcript_fingerprint
                == AgentTranscriptFingerprintAccumulator::default().fingerprint())
            .then(Vec::new);
        }

        // Context compaction replaces an older durable prefix with a transient
        // restore pack. The remaining durable messages are therefore a suffix
        // of the event transcript, not necessarily the whole transcript.
        let first_durable = durable_indices[durable_indices.len() - self.durable_message_count];
        let projection = messages[first_durable..]
            .iter()
            .filter(|message| !is_transient_run_context(message))
            .cloned()
            .collect::<Vec<_>>();
        let durable_projection = durable_messages(&projection);
        (durable_projection.len() == self.durable_message_count
            && self.transcript_fingerprint
                == AgentTranscriptFingerprintAccumulator::from_messages(
                    durable_projection.iter().copied(),
                )
                .fingerprint())
        .then_some(projection)
    }

    pub fn to_json(&self) -> Result<String, AgentTaskStateError> {
        let encoded = serde_json::to_string(self).map_err(|error| {
            AgentTaskStateError::new(format!("failed to encode agent task checkpoint: {error}"))
        })?;
        if encoded.len() > MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES {
            return Err(AgentTaskStateError::new(format!(
                "agent task checkpoint exceeds the {MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES}-byte serialized size limit"
            )));
        }
        Ok(encoded)
    }

    pub fn from_json(encoded: &str) -> Result<Self, AgentTaskStateError> {
        if encoded.len() > MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES {
            return Err(AgentTaskStateError::new(format!(
                "agent task checkpoint exceeds the {MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES}-byte serialized size limit"
            )));
        }
        let snapshot = serde_json::from_str::<Self>(encoded).map_err(|error| {
            AgentTaskStateError::new(format!("failed to decode agent task checkpoint: {error}"))
        })?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate_checkpoint(&self) -> Result<(), AgentTaskStateError> {
        self.validate()
    }

    pub(crate) fn validate(&self) -> Result<(), AgentTaskStateError> {
        match self.schema.as_str() {
            AGENT_TASK_STATE_SCHEMA => self
                .prepared_task_state
                .as_ref()
                .ok_or_else(|| {
                    AgentTaskStateError::new(
                        "v2 agent task checkpoint is missing prepared task state",
                    )
                })?
                .validate()?,
            AGENT_TASK_STATE_SCHEMA_V1 if self.prepared_task_state.is_none() => {}
            AGENT_TASK_STATE_SCHEMA_V1 => {
                return Err(AgentTaskStateError::new(
                    "v1 agent task checkpoint contains unsupported prepared task state",
                ))
            }
            _ => {
                return Err(AgentTaskStateError::new(format!(
                    "unsupported agent task checkpoint schema: {}",
                    self.schema
                )))
            }
        }
        if self.task_id.trim().is_empty() {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint is missing a task id",
            ));
        }
        if self.max_turns == 0 {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint has an invalid turn budget",
            ));
        }
        self.task_contract
            .validate_persisted_outcome_state()
            .map_err(|error| {
                AgentTaskStateError::new(format!(
                    "agent task checkpoint contains invalid outcome state: {error}"
                ))
            })?;
        let (steer_epoch, contract_epoch) = self
            .prepared_task_state
            .as_ref()
            .map(|prepared| (prepared.steer_epoch, prepared.contract_epoch))
            .unwrap_or((0, 0));
        self.task_contract
            .validate_persisted_postcondition_state(steer_epoch, contract_epoch)
            .map_err(|error| {
                AgentTaskStateError::new(format!(
                    "agent task checkpoint contains invalid postcondition state: {error}"
                ))
            })?;
        Ok(())
    }
}

fn durable_messages(messages: &[Message]) -> Vec<&Message> {
    messages
        .iter()
        .filter(|message| is_durable_message(message))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        record_tool_outcome, record_tool_outcome_with_risk, repeated_tool_failure_count,
        start_agent_loop, tool_input_fingerprint, AgentActionDenialFeedback, AgentRuntimeConfig,
        InteractionSurface, OutcomeSatisfaction, PromptCompletionIntent,
        WorkspaceVerificationPolicy, TOOL_FAILURE_SIGNATURE_SCHEMA,
    };
    use agent_core::{MessageRole, Metadata, TaskId, ToolOutcomeStatus, ToolRisk};

    #[test]
    fn round_trips_control_state_against_the_durable_transcript() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig { max_turns: 32 },
        );
        state.turn = 7;
        for _ in 0..2 {
            record_tool_outcome(
                &mut state,
                "shell.run",
                r#"{"command":"false","cwd":"."}"#,
                &ToolOutcomeStatus::Failed,
            );
        }
        state.task_contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        let claim_sentinel = "RECOVERY_CLAIM_SENTINEL";
        state.task_contract.observe_completion_candidate(
            3,
            state.turn,
            claim_sentinel,
            crate::OutcomeClaimDecision::RepairRequired,
        );
        for path in ["src/one.rs", "src/two.rs", "src/three.rs"] {
            record_tool_outcome_with_risk(
                &mut state,
                "file.write",
                &format!(r#"{{"path":"{path}"}}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::WritesWorkspace),
            );
        }
        record_tool_outcome(
            &mut state,
            "browser.click",
            r##"{"selector":"#submit"}"##,
            &ToolOutcomeStatus::Succeeded,
        );
        let encoded = AgentTaskStateSnapshot::capture(&state)
            .to_json()
            .expect("checkpoint encodes");
        assert!(encoded.len() <= MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES);
        assert!(!encoded.contains(claim_sentinel));
        let snapshot = AgentTaskStateSnapshot::from_json(&encoded).expect("checkpoint decodes");
        let restored = snapshot
            .restore(state.user_prompt.clone(), state.messages.clone())
            .expect("matching transcript restores");

        assert_eq!(restored, state);
    }

    #[test]
    fn serialized_snapshot_limit_rejects_oversized_encode_and_precedes_decode() {
        let state = start_agent_loop(
            TaskId("snapshot-size-limit".to_string()),
            "validate checkpoint size",
            AgentRuntimeConfig::default(),
        );
        let mut oversized = AgentTaskStateSnapshot::capture(&state);
        oversized.task_id = "x".repeat(MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES);

        let encode_error = oversized
            .to_json()
            .expect_err("oversized checkpoint must not encode");
        assert!(encode_error.to_string().contains("serialized size limit"));

        let invalid_oversized_json = "x".repeat(MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES + 1);
        let decode_error = AgentTaskStateSnapshot::from_json(&invalid_oversized_json)
            .expect_err("oversized input must be rejected before decoding");
        assert!(decode_error.to_string().contains("serialized size limit"));
        assert!(!decode_error
            .to_string()
            .contains("failed to decode agent task checkpoint"));
    }

    #[test]
    fn v2_persists_typed_preparation_without_raw_objective_or_targets() {
        let mut state = start_agent_loop(
            TaskId("task-v2".to_string()),
            "initial request",
            AgentRuntimeConfig::default(),
        );
        let effective_objective =
            "Inspect crates/secret-target.rs and https://example.com/private-target";
        let run_context = [
            (
                "effective_prompt_objective".to_string(),
                effective_objective.to_string(),
            ),
            ("steer_epoch".to_string(), "5".to_string()),
            ("prompt_contract_epoch".to_string(), "4".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let prepared = PreparedTaskState::from_run_context(
            &run_context,
            "initial request",
            crate::prompt_completion_intent(&run_context),
        );
        assert!(!prepared.completion_intent().target_anchors.is_empty());
        state.replace_prepared_task_state(prepared.clone());

        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let encoded = snapshot.to_json().expect("v2 checkpoint encodes");
        let value = serde_json::from_str::<serde_json::Value>(&encoded).expect("valid JSON");

        assert_eq!(value["schema"], AGENT_TASK_STATE_SCHEMA);
        assert!(value.get("successfulMutations").is_none());
        assert!(value.get("verifiedAfterLastMutation").is_none());
        assert!(value.get("pendingInteractionVerifications").is_none());
        assert!(!encoded.contains(effective_objective));
        assert!(!encoded.contains("secret-target.rs"));
        assert!(!encoded.contains("private-target"));
        assert_eq!(
            value["preparedTaskState"]["objectiveFingerprint"],
            text_fingerprint(effective_objective)
        );
        assert_eq!(value["preparedTaskState"]["steerEpoch"], 5);
        assert_eq!(value["preparedTaskState"]["contractEpoch"], 4);

        let restored = AgentTaskStateSnapshot::from_json(&encoded)
            .expect("v2 checkpoint decodes")
            .restore_with_prepared_task_state(
                state.user_prompt.clone(),
                state.messages.clone(),
                prepared,
            )
            .expect("supplied prepared objective restores");
        assert_eq!(restored, state);
    }

    #[test]
    fn v2_checkpoint_preserves_denial_state_without_raw_tool_input() {
        let mut state = start_agent_loop(
            TaskId("task-denial".to_string()),
            "write protected.txt",
            AgentRuntimeConfig::default(),
        );
        state.task_contract.begin_action_denial_epoch(0);
        state.task_contract.require_tool_success("file.write");
        let raw_input = r#"{"path":"private-checkpoint-sentinel.txt"}"#;
        state.task_contract.record_action_denial(
            "file.write",
            &tool_input_fingerprint("file.write", raw_input),
            &AgentActionDenialFeedback::user_permission(),
        );

        let encoded = AgentTaskStateSnapshot::capture(&state)
            .to_json()
            .expect("denial checkpoint encodes");
        assert!(!encoded.contains("private-checkpoint-sentinel"));
        let restored = AgentTaskStateSnapshot::from_json(&encoded)
            .expect("denial checkpoint validates")
            .restore(state.user_prompt.clone(), state.messages.clone())
            .expect("denial checkpoint restores");
        let ledger = restored.task_contract.outcome_ledger_shadow(0);
        assert_eq!(ledger.obligations.len(), 1);
        assert_eq!(
            ledger.obligations[0].satisfaction,
            OutcomeSatisfaction::Blocked
        );
    }

    #[test]
    fn v2_reconstructs_runtime_only_target_anchors_from_the_effective_objective() {
        let objective = "Inspect crates/agent-runtime/src/task_state.rs";
        let state = start_agent_loop(
            TaskId("task-v2-targets".to_string()),
            objective,
            AgentRuntimeConfig::default(),
        );
        assert!(!state
            .prepared_task_state()
            .completion_intent()
            .target_anchors
            .is_empty());

        let restored = AgentTaskStateSnapshot::from_json(
            &AgentTaskStateSnapshot::capture(&state)
                .to_json()
                .expect("checkpoint encodes"),
        )
        .expect("checkpoint decodes")
        .restore(objective, state.messages.clone())
        .expect("matching objective restores");

        assert_eq!(
            restored
                .prepared_task_state()
                .completion_intent()
                .target_anchors,
            state
                .prepared_task_state()
                .completion_intent()
                .target_anchors
        );
    }

    #[test]
    fn v2_restores_a_steered_effective_objective_from_persisted_typed_facts() {
        let user_prompt = "Inspect the project";
        let effective_objective =
            "Initial request:\nInspect the project\n\nAccepted steering 1:\nInspect crates/agent-runtime/src/task_state.rs";
        let run_context = [
            (
                "effective_prompt_objective".to_string(),
                effective_objective.to_string(),
            ),
            ("steer_epoch".to_string(), "5".to_string()),
            ("prompt_contract_epoch".to_string(), "4".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut state = start_agent_loop(
            TaskId("task-v2-steered".to_string()),
            user_prompt,
            AgentRuntimeConfig::default(),
        );
        state.replace_prepared_task_state(PreparedTaskState::from_run_context(
            &run_context,
            user_prompt,
            crate::prompt_completion_intent(&run_context),
        ));
        let checkpoint = AgentTaskStateSnapshot::from_json(
            &AgentTaskStateSnapshot::capture(&state)
                .to_json()
                .expect("checkpoint encodes"),
        )
        .expect("checkpoint decodes");

        let restored = checkpoint
            .restore_with_effective_objective(
                user_prompt,
                state.messages.clone(),
                effective_objective,
            )
            .expect("steered checkpoint restores");
        assert_eq!(restored.prepared_task_state().steer_epoch(), 5);
        assert_eq!(restored.prepared_task_state().contract_epoch(), 4);
        assert_eq!(
            restored.prepared_task_state().completion_intent(),
            state.prepared_task_state().completion_intent()
        );

        let error = checkpoint
            .restore_with_effective_objective(
                user_prompt,
                state.messages.clone(),
                "Inspect a different target",
            )
            .expect_err("a different effective objective must fail closed");
        assert!(error.to_string().contains("prepared task state"));
    }

    #[test]
    fn explicit_v1_decoder_reconstructs_legacy_contract_mirrors() {
        let state = start_agent_loop(
            TaskId("task-v1".to_string()),
            "resume legacy work",
            AgentRuntimeConfig::default(),
        );
        let mut value = serde_json::to_value(AgentTaskStateSnapshot::capture(&state))
            .expect("v2 checkpoint converts to JSON");
        value["schema"] = AGENT_TASK_STATE_SCHEMA_V1.into();
        value
            .as_object_mut()
            .expect("checkpoint object")
            .remove("preparedTaskState");
        value["successfulMutations"] = 2.into();
        value["verifiedAfterLastMutation"] = true.into();
        value["pendingInteractionVerifications"] = serde_json::json!([{
            "surface": "browser",
            "actionTool": "browser.click"
        }]);
        value["taskContract"] =
            serde_json::to_value(AgentTaskContract::default()).expect("default contract encodes");
        let encoded = serde_json::to_string(&value).expect("v1 fixture encodes");
        assert!(encoded.len() <= MAX_AGENT_TASK_STATE_SNAPSHOT_BYTES);

        let snapshot = AgentTaskStateSnapshot::from_json(&encoded).expect("v1 decoder accepts");
        snapshot
            .validate_checkpoint()
            .expect("explicitly migrated v1 checkpoint validates");
        assert_eq!(snapshot.schema, AGENT_TASK_STATE_SCHEMA_V1);
        assert!(snapshot.prepared_task_state.is_none());
        let restored = snapshot
            .restore_with_prepared_task_state(
                state.user_prompt.clone(),
                state.messages.clone(),
                state.prepared_task_state().clone(),
            )
            .expect("legacy checkpoint restores through typed state");

        assert_eq!(restored.successful_mutations(), 2);
        assert!(restored.verified_after_last_mutation());
        assert_eq!(
            restored
                .pending_interaction_verifications()
                .get(&InteractionSurface::Browser)
                .map(String::as_str),
            Some("browser.click")
        );
    }

    #[test]
    fn checkpoint_schema_and_prepared_state_mismatches_fail_closed() {
        let state = start_agent_loop(
            TaskId("task-schema".to_string()),
            "inspect the project",
            AgentRuntimeConfig::default(),
        );
        let mut value = serde_json::to_value(AgentTaskStateSnapshot::capture(&state))
            .expect("checkpoint converts to JSON");

        value["schema"] = "cindx.agent.task-state.v999".into();
        let unknown = serde_json::to_string(&value).expect("unknown fixture encodes");
        assert!(AgentTaskStateSnapshot::from_json(&unknown)
            .unwrap_err()
            .to_string()
            .contains("unsupported agent task checkpoint schema"));

        value["schema"] = AGENT_TASK_STATE_SCHEMA.into();
        value
            .as_object_mut()
            .expect("checkpoint object")
            .remove("preparedTaskState");
        let missing = serde_json::to_string(&value).expect("missing fixture encodes");
        assert!(AgentTaskStateSnapshot::from_json(&missing).is_err());

        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let mismatched = PreparedTaskState::from_run_context(
            &[(
                "effective_prompt_objective".to_string(),
                "different objective".to_string(),
            )]
            .into_iter()
            .collect(),
            "different objective",
            PromptCompletionIntent::default(),
        );
        assert!(snapshot
            .restore_with_prepared_task_state(
                state.user_prompt.clone(),
                state.messages.clone(),
                mismatched,
            )
            .unwrap_err()
            .to_string()
            .contains("prepared task state"));
    }

    #[test]
    fn rejects_corrupt_or_unbounded_persisted_outcome_claims() {
        let mut state = start_agent_loop(
            TaskId("outcome-state-validation".to_string()),
            "validate outcome state",
            AgentRuntimeConfig::default(),
        );
        state.task_contract.observe_completion_candidate(
            1,
            1,
            "candidate",
            crate::OutcomeClaimDecision::Accepted,
        );
        let base = serde_json::to_value(AgentTaskStateSnapshot::capture(&state))
            .expect("checkpoint converts to JSON");

        let mut invalid_states = Vec::new();

        let mut too_many = base.clone();
        let claims = too_many["taskContract"]["outcomeClaims"]
            .as_array_mut()
            .expect("claims array");
        let template = claims[0].clone();
        for sequence in 2..=9 {
            let mut claim = template.clone();
            claim["sequence"] = sequence.into();
            claims.push(claim);
        }
        too_many["taskContract"]["nextOutcomeClaimSequence"] = 9.into();
        invalid_states.push(too_many);

        let mut duplicate = base.clone();
        let duplicate_claim = duplicate["taskContract"]["outcomeClaims"][0].clone();
        duplicate["taskContract"]["outcomeClaims"]
            .as_array_mut()
            .expect("claims array")
            .push(duplicate_claim);
        duplicate["taskContract"]["nextOutcomeClaimSequence"] = 2.into();
        invalid_states.push(duplicate);

        for (field, value) in [
            (
                "contentSha256",
                serde_json::Value::String("bad".to_string()),
            ),
            ("contentBytes", serde_json::Value::Number(0.into())),
            (
                "kind",
                serde_json::Value::String("delivered_answer".to_string()),
            ),
            (
                "decision",
                serde_json::Value::String("delivered".to_string()),
            ),
            (
                "evidenceStatus",
                serde_json::Value::String("available_not_entailed".to_string()),
            ),
            ("availableEvidenceSequences", serde_json::json!([1])),
        ] {
            let mut invalid = base.clone();
            invalid["taskContract"]["outcomeClaims"][0][field] = value;
            invalid_states.push(invalid);
        }

        let mut cursor_behind = base.clone();
        cursor_behind["taskContract"]["nextOutcomeClaimSequence"] = 0.into();
        invalid_states.push(cursor_behind);

        let mut dropped_inconsistent = base.clone();
        dropped_inconsistent["taskContract"]["outcomeDroppedClaims"] = 99.into();
        invalid_states.push(dropped_inconsistent);

        for invalid in invalid_states {
            let encoded = serde_json::to_string(&invalid).expect("invalid checkpoint encodes");
            let error = AgentTaskStateSnapshot::from_json(&encoded)
                .expect_err("invalid outcome state must be rejected");
            assert!(error.to_string().contains("invalid outcome state"));
        }

        let mut legacy = base;
        let contract = legacy["taskContract"]
            .as_object_mut()
            .expect("task contract object");
        contract.remove("outcomeClaims");
        contract.remove("outcomeDroppedClaims");
        contract.remove("nextOutcomeClaimSequence");
        let encoded = serde_json::to_string(&legacy).expect("legacy checkpoint encodes");
        AgentTaskStateSnapshot::from_json(&encoded)
            .expect("checkpoint without outcome fields remains compatible");
    }

    #[test]
    fn legacy_raw_failure_keys_are_sanitized_without_losing_the_counter() {
        let mut state = start_agent_loop(
            TaskId("legacy-failure-checkpoint".to_string()),
            "recover a failed tool",
            AgentRuntimeConfig::default(),
        );
        let input = r#"{"command":"printenv TOP_SECRET_TOKEN","cwd":"."}"#;
        state
            .failed_tool_signatures
            .insert(format!("shell.run\n{input}"), 2);

        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let encoded = snapshot.to_json().expect("checkpoint encodes");

        assert!(!encoded.contains("TOP_SECRET_TOKEN"));
        assert!(snapshot
            .failed_tool_signatures
            .keys()
            .all(|signature| signature.starts_with(&format!("{TOOL_FAILURE_SIGNATURE_SCHEMA}:"))));

        let restored = AgentTaskStateSnapshot::from_json(&encoded)
            .expect("checkpoint decodes")
            .restore(state.user_prompt.clone(), state.messages.clone())
            .expect("matching transcript restores");
        assert_eq!(
            repeated_tool_failure_count(&restored, "shell.run", input),
            2
        );
    }

    #[test]
    fn recovery_only_messages_do_not_break_transcript_lineage() {
        let state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let mut messages = state.messages.clone();
        messages.push(Message {
            role: MessageRole::Tool,
            content: "prior tool outcome is unknown".to_string(),
            metadata: [("kind".to_string(), "recovery_observation".to_string())]
                .into_iter()
                .collect::<Metadata>(),
        });

        let restored = snapshot
            .restore(state.user_prompt.clone(), messages.clone())
            .expect("synthetic recovery message is additive");
        assert_eq!(restored.messages, messages);
    }

    #[test]
    fn transient_run_context_does_not_enter_checkpoint_lineage() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        state.messages.insert(
            0,
            Message {
                role: MessageRole::System,
                content: "request-scoped retrieved evidence".to_string(),
                metadata: [
                    ("internal".to_string(), "true".to_string()),
                    ("kind".to_string(), "knowledge_context".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        );
        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let durable_transcript = state.messages[1..].to_vec();

        let restored = snapshot
            .restore(state.user_prompt.clone(), state.messages.clone())
            .expect("request-scoped context should be regenerated, not persisted");
        assert_eq!(restored.messages, durable_transcript);
    }

    #[test]
    fn compacted_checkpoint_matches_a_verified_transcript_suffix() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "continue the implementation",
            AgentRuntimeConfig::default(),
        );
        let old_message = Message {
            role: MessageRole::Assistant,
            content: "old completed discussion".to_string(),
            metadata: Metadata::new(),
        };
        let recent_message = Message {
            role: MessageRole::Assistant,
            content: "recent verified work".to_string(),
            metadata: Metadata::new(),
        };
        state.messages = vec![recent_message.clone()];
        let snapshot = AgentTaskStateSnapshot::capture(&state);

        let restored = snapshot
            .restore(
                state.user_prompt.clone(),
                vec![old_message, recent_message.clone()],
            )
            .expect("compacted durable suffix should restore");
        assert_eq!(restored.messages, vec![recent_message]);
    }

    #[test]
    fn durable_internal_instructions_remain_in_checkpoint_lineage() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        state.messages.push(Message {
            role: MessageRole::System,
            content: "verify before completion".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "completion_verification".to_string()),
            ]
            .into_iter()
            .collect(),
        });
        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let mut missing_instruction = state.messages.clone();
        missing_instruction.pop();

        assert!(snapshot
            .restore(state.user_prompt.clone(), missing_instruction)
            .unwrap_err()
            .to_string()
            .contains("durable transcript"));
    }

    #[test]
    fn rejects_stale_or_tampered_transcripts() {
        let state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let mut messages = state.messages.clone();
        messages[0].content = "different request".to_string();

        assert!(snapshot
            .restore(state.user_prompt.clone(), messages)
            .unwrap_err()
            .to_string()
            .contains("durable transcript"));
        assert!(snapshot
            .restore("different prompt", state.messages.clone())
            .unwrap_err()
            .to_string()
            .contains("active user prompt"));
    }
}
