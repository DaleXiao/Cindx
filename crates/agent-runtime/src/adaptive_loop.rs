use crate::{tool_input_fingerprint, AgentGoalDelta, AgentLoopState};
use agent_core::{
    ToolEffectSemantics, ToolObservationV2, ToolOutcomeStatus, ToolResult,
    TOOL_OBSERVATION_V2_SCHEMA,
};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_ADAPTIVE_NO_GAIN_COUNT: u8 = 3;
pub const ADAPTIVE_REPLAN_NO_GAIN_COUNT: u8 = 2;

const MAX_OBSERVATION_FACTS: usize = 24;
const MAX_FACT_KEY_BYTES: usize = 96;
const MAX_FACT_VALUE_BYTES: usize = 384;
const MAX_TOOL_NAME_BYTES: usize = 128;
const MAX_FAILURE_CODE_BYTES: usize = 128;
const MAX_SEMANTIC_EVIDENCE_BYTES: usize = 32 * 1024;
const MAX_SEMANTIC_SUMMARY_BYTES: usize = 2 * 1024;
const MAX_SEMANTIC_NEXT_ACTION_BYTES: usize = 2 * 1024;
pub(crate) const MAX_RECENT_SEMANTIC_ACTIONS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecentSemanticObservation {
    action_sha256: [u8; 32],
    outcome_sha256: [u8; 32],
}

/// A bounded control cursor over trusted, typed tool observations.
///
/// This is deliberately not a second source of task facts. It retains only
/// fixed-size hashes and counters; task progress remains authoritative in the
/// task contract and its goal-delta receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AdaptiveLoopCursor {
    steer_epoch: u64,
    prior_action_sha256: Option<[u8; 32]>,
    prior_outcome_sha256: Option<[u8; 32]>,
    #[serde(deserialize_with = "deserialize_no_gain_count")]
    no_gain_count: u8,
    replan_emitted: bool,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_semantic_history"
    )]
    recent_semantic_observations: Vec<RecentSemanticObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdaptiveLoopDisposition {
    Continue,
    ReplanOnce,
    CommitTerminalResult,
}

impl AdaptiveLoopCursor {
    pub fn for_steer_epoch(steer_epoch: u64) -> Self {
        Self {
            steer_epoch,
            ..Self::default()
        }
    }

    pub fn steer_epoch(&self) -> u64 {
        self.steer_epoch
    }

    pub fn no_gain_count(&self) -> u8 {
        self.no_gain_count
    }

    pub fn replan_emitted(&self) -> bool {
        self.replan_emitted
    }

    #[cfg(test)]
    pub(crate) fn semantic_history_len(&self) -> usize {
        self.recent_semantic_observations.len()
    }

    pub fn disposition(&self) -> AdaptiveLoopDisposition {
        if self.replan_emitted && self.no_gain_count >= MAX_ADAPTIVE_NO_GAIN_COUNT {
            AdaptiveLoopDisposition::CommitTerminalResult
        } else if self.replan_emitted && self.no_gain_count >= ADAPTIVE_REPLAN_NO_GAIN_COUNT {
            AdaptiveLoopDisposition::ReplanOnce
        } else {
            AdaptiveLoopDisposition::Continue
        }
    }

    pub fn reset_for_steer(&mut self, steer_epoch: u64) {
        self.steer_epoch = steer_epoch;
        self.clear_epoch_state();
    }

    pub fn reset_for_goal_delta(&mut self, steer_epoch: u64, _delta: &AgentGoalDelta) {
        self.steer_epoch = steer_epoch;
        self.clear_epoch_state();
    }

    /// Observe one completed tool attempt and return an advisory loop action.
    ///
    /// Untyped/legacy observations clear advisory state so unknown work cannot
    /// bridge two otherwise identical observations into a terminal decision.
    /// A goal delta or steer epoch change resets the cursor before the current
    /// typed observation becomes the new baseline. Incomplete evidence is
    /// likewise a continuation signal and cannot add to the no-gain count.
    pub fn observe_tool_result(
        &mut self,
        steer_epoch: u64,
        tool_name: &str,
        input_fingerprint: &str,
        result: &ToolResult,
        effect_semantics: Option<&ToolEffectSemantics>,
        goal_delta: Option<&AgentGoalDelta>,
    ) -> AdaptiveLoopDisposition {
        let reset = steer_epoch != self.steer_epoch || goal_delta.is_some();
        if reset {
            self.steer_epoch = steer_epoch;
            self.clear_epoch_state();
        }

        let Some(observation) = trusted_observation(result) else {
            self.clear_epoch_state();
            return AdaptiveLoopDisposition::Continue;
        };
        let action_sha256 = action_fingerprint(tool_name, input_fingerprint);
        let outcome_sha256 = outcome_fingerprint(result, observation);
        let semantic_read_eligible =
            semantic_read_is_eligible(effect_semantics, result, observation);
        let semantic_outcome_sha256 =
            semantic_read_outcome_fingerprint(effect_semantics, result, observation);

        if reset {
            if let Some(semantic_outcome_sha256) = semantic_outcome_sha256 {
                self.record_semantic_observation(action_sha256, semantic_outcome_sha256);
            } else {
                self.recent_semantic_observations.clear();
            }
            self.set_baseline(action_sha256, outcome_sha256);
            self.clear_no_gain_signal();
            return AdaptiveLoopDisposition::Continue;
        }

        if !observation.evidence_complete {
            self.recent_semantic_observations.clear();
            self.set_baseline(action_sha256, outcome_sha256);
            self.clear_no_gain_signal();
            return AdaptiveLoopDisposition::Continue;
        }

        if let Some(semantic_outcome_sha256) = semantic_outcome_sha256 {
            let semantic_repeated =
                self.record_semantic_observation(action_sha256, semantic_outcome_sha256);
            self.set_baseline(action_sha256, outcome_sha256);
            if semantic_repeated {
                if self.replan_emitted {
                    self.no_gain_count = MAX_ADAPTIVE_NO_GAIN_COUNT;
                    return AdaptiveLoopDisposition::CommitTerminalResult;
                }
                self.no_gain_count = ADAPTIVE_REPLAN_NO_GAIN_COUNT;
                self.replan_emitted = true;
                return AdaptiveLoopDisposition::ReplanOnce;
            }
            self.clear_no_gain_signal();
            return AdaptiveLoopDisposition::Continue;
        }
        self.recent_semantic_observations.clear();
        if semantic_read_eligible {
            self.set_baseline(action_sha256, outcome_sha256);
            self.clear_no_gain_signal();
            return AdaptiveLoopDisposition::Continue;
        }

        let repeated = self.prior_action_sha256 == Some(action_sha256)
            && self.prior_outcome_sha256 == Some(outcome_sha256);
        self.set_baseline(action_sha256, outcome_sha256);
        if !repeated {
            self.clear_no_gain_signal();
            return AdaptiveLoopDisposition::Continue;
        }

        self.no_gain_count = self
            .no_gain_count
            .saturating_add(1)
            .min(MAX_ADAPTIVE_NO_GAIN_COUNT);
        if !self.replan_emitted && self.no_gain_count >= ADAPTIVE_REPLAN_NO_GAIN_COUNT {
            self.replan_emitted = true;
            return AdaptiveLoopDisposition::ReplanOnce;
        }
        if self.replan_emitted && self.no_gain_count >= MAX_ADAPTIVE_NO_GAIN_COUNT {
            return AdaptiveLoopDisposition::CommitTerminalResult;
        }
        AdaptiveLoopDisposition::Continue
    }

    fn clear_epoch_state(&mut self) {
        self.prior_action_sha256 = None;
        self.prior_outcome_sha256 = None;
        self.recent_semantic_observations.clear();
        self.clear_no_gain_signal();
    }

    fn clear_for_missing_tool_result(&mut self) {
        self.clear_epoch_state();
    }

    fn set_baseline(&mut self, action_sha256: [u8; 32], outcome_sha256: [u8; 32]) {
        self.prior_action_sha256 = Some(action_sha256);
        self.prior_outcome_sha256 = Some(outcome_sha256);
    }

    fn clear_no_gain_signal(&mut self) {
        self.no_gain_count = 0;
        self.replan_emitted = false;
    }

    fn record_semantic_observation(
        &mut self,
        action_sha256: [u8; 32],
        outcome_sha256: [u8; 32],
    ) -> bool {
        let existing = self
            .recent_semantic_observations
            .iter()
            .position(|observation| observation.action_sha256 == action_sha256);
        let repeated = existing.is_some_and(|index| {
            self.recent_semantic_observations[index].outcome_sha256 == outcome_sha256
        });
        if let Some(index) = existing {
            self.recent_semantic_observations.remove(index);
        } else if self.recent_semantic_observations.len() >= MAX_RECENT_SEMANTIC_ACTIONS {
            self.recent_semantic_observations.remove(0);
        }
        self.recent_semantic_observations
            .push(RecentSemanticObservation {
                action_sha256,
                outcome_sha256,
            });
        repeated
    }
}

impl AgentLoopState {
    pub fn adaptive_loop_disposition(&self) -> AdaptiveLoopDisposition {
        self.adaptive_loop_cursor.disposition()
    }

    pub(crate) fn observe_adaptive_tool_result(
        &mut self,
        tool_name: &str,
        input: &str,
        result: &ToolResult,
        effect_semantics: Option<&ToolEffectSemantics>,
        goal_delta: Option<&AgentGoalDelta>,
    ) -> AdaptiveLoopDisposition {
        let input_fingerprint = tool_input_fingerprint(tool_name, input);
        self.adaptive_loop_cursor.observe_tool_result(
            self.prepared_task_state.steer_epoch(),
            tool_name,
            &input_fingerprint,
            result,
            effect_semantics,
            goal_delta,
        )
    }

    pub(crate) fn clear_adaptive_state_for_missing_tool_result(&mut self) {
        self.adaptive_loop_cursor.clear_for_missing_tool_result();
    }
}

fn trusted_observation(result: &ToolResult) -> Option<&ToolObservationV2> {
    result
        .model_observation
        .as_ref()
        .filter(|observation| observation.schema == TOOL_OBSERVATION_V2_SCHEMA)
}

fn action_fingerprint(tool_name: &str, input_fingerprint: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"cindx.adaptive-action.v1\0");
    update_bounded(&mut digest, tool_name.as_bytes(), MAX_TOOL_NAME_BYTES);
    update_bounded(&mut digest, input_fingerprint.as_bytes(), 128);
    digest.finalize().into()
}

fn outcome_fingerprint(result: &ToolResult, observation: &ToolObservationV2) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"cindx.adaptive-outcome.v1\0");
    digest.update([
        status_tag(&result.status),
        u8::from(observation.evidence_complete),
    ]);
    digest.update((observation.facts.len() as u64).to_be_bytes());
    for (key, value) in observation.facts.iter().take(MAX_OBSERVATION_FACTS) {
        update_bounded(&mut digest, key.as_bytes(), MAX_FACT_KEY_BYTES);
        update_bounded(&mut digest, value.as_bytes(), MAX_FACT_VALUE_BYTES);
    }
    match &result.failure {
        Some(failure) => {
            digest.update([1, u8::from(failure.retryable)]);
            update_bounded(&mut digest, failure.code.as_bytes(), MAX_FAILURE_CODE_BYTES);
        }
        None => digest.update([0, 0]),
    }
    digest.finalize().into()
}

fn semantic_read_outcome_fingerprint(
    effect_semantics: Option<&ToolEffectSemantics>,
    result: &ToolResult,
    observation: &ToolObservationV2,
) -> Option<[u8; 32]> {
    if !semantic_read_is_eligible(effect_semantics, result, observation) {
        return None;
    }
    if observation.summary.len() > MAX_SEMANTIC_SUMMARY_BYTES
        || observation
            .next_action
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SEMANTIC_NEXT_ACTION_BYTES)
        || observation.facts.len() > MAX_OBSERVATION_FACTS
    {
        return None;
    }

    let evidence = observation.evidence.as_bytes();
    let direct_evidence = !evidence.is_empty() && evidence.len() <= MAX_SEMANTIC_EVIDENCE_BYTES;
    let mut digest = Sha256::new();
    digest.update(b"cindx.adaptive-semantic-read.v2\0");
    update_bounded(
        &mut digest,
        observation.summary.as_bytes(),
        MAX_SEMANTIC_SUMMARY_BYTES,
    );
    match observation.next_action.as_deref() {
        Some(next_action) => {
            digest.update([1]);
            update_bounded(
                &mut digest,
                next_action.as_bytes(),
                MAX_SEMANTIC_NEXT_ACTION_BYTES,
            );
        }
        None => digest.update([0]),
    }

    let mut digest_fact_count = 0_u8;
    for (key, value) in &observation.facts {
        if is_volatile_semantic_fact(key) {
            continue;
        }
        if key.len() > MAX_FACT_KEY_BYTES || value.len() > MAX_FACT_VALUE_BYTES {
            return None;
        }
        let is_digest = key == "sha256" || key.ends_with("_sha256");
        if is_digest && !is_lower_hex_sha256(value) {
            return None;
        }
        update_bounded(&mut digest, key.as_bytes(), MAX_FACT_KEY_BYTES);
        update_bounded(&mut digest, value.as_bytes(), MAX_FACT_VALUE_BYTES);
        if is_digest {
            digest_fact_count = digest_fact_count.saturating_add(1);
        }
    }
    if direct_evidence {
        digest.update([1]);
        update_bounded(&mut digest, evidence, MAX_SEMANTIC_EVIDENCE_BYTES);
    } else if digest_fact_count > 0 {
        digest.update([2, digest_fact_count]);
    } else {
        return None;
    }
    Some(digest.finalize().into())
}

fn semantic_read_is_eligible(
    effect_semantics: Option<&ToolEffectSemantics>,
    result: &ToolResult,
    observation: &ToolObservationV2,
) -> bool {
    matches!(effect_semantics, Some(ToolEffectSemantics::ReadOnly))
        && matches!(result.status, ToolOutcomeStatus::Succeeded)
        && observation.evidence_complete
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn is_volatile_semantic_fact(key: &str) -> bool {
    matches!(
        key,
        "artifact_path" | "duration_ms" | "text_path" | "trace_path"
    )
}

fn update_bounded(digest: &mut Sha256, bytes: &[u8], limit: usize) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(&bytes[..bytes.len().min(limit)]);
}

fn status_tag(status: &ToolOutcomeStatus) -> u8 {
    match status {
        ToolOutcomeStatus::Succeeded => 0,
        ToolOutcomeStatus::Failed => 1,
        ToolOutcomeStatus::Cancelled => 2,
        ToolOutcomeStatus::Denied => 3,
    }
}

fn deserialize_no_gain_count<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(u8::deserialize(deserializer)?.min(MAX_ADAPTIVE_NO_GAIN_COUNT))
}

fn deserialize_semantic_history<'de, D>(
    deserializer: D,
) -> Result<Vec<RecentSemanticObservation>, D::Error>
where
    D: Deserializer<'de>,
{
    let observations = Vec::<RecentSemanticObservation>::deserialize(deserializer)?;
    if observations.len() > MAX_RECENT_SEMANTIC_ACTIONS {
        return Err(<D::Error as serde::de::Error>::custom(
            "adaptive semantic history exceeds its fixed bound",
        ));
    }
    Ok(observations)
}

#[cfg(test)]
#[path = "adaptive_loop/semantic_tests.rs"]
mod semantic_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Metadata, ToolCallId, ToolFailure};

    const INPUT_A: &str = concat!(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    const INPUT_B: &str = concat!(
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );

    fn result(status: ToolOutcomeStatus, complete: bool, facts: &[(&str, &str)]) -> ToolResult {
        let facts = facts
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        ToolResult {
            invocation_id: ToolCallId("call-1".to_string()),
            status,
            output: "excluded output".to_string(),
            content: Vec::new(),
            structured_output_json: Some("excluded structured output".to_string()),
            artifacts: Vec::new(),
            failure: None,
            model_observation: Some(ToolObservationV2::new(
                "workspace.read",
                "excluded summary",
                "excluded evidence",
                complete,
                facts,
            )),
            metadata: Metadata::new(),
        }
    }

    fn observe(
        cursor: &mut AdaptiveLoopCursor,
        steer_epoch: u64,
        result: &ToolResult,
        goal_delta: Option<&AgentGoalDelta>,
    ) -> AdaptiveLoopDisposition {
        cursor.observe_tool_result(
            steer_epoch,
            "workspace.read",
            INPUT_A,
            result,
            Some(&ToolEffectSemantics::NonIdempotent),
            goal_delta,
        )
    }

    #[test]
    fn exact_complete_no_gain_replans_once_then_commits() {
        let mut cursor = AdaptiveLoopCursor::for_steer_epoch(4);
        let result = result(ToolOutcomeStatus::Succeeded, true, &[("revision", "1")]);

        assert_eq!(
            observe(&mut cursor, 4, &result, None),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(
            observe(&mut cursor, 4, &result, None),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(
            observe(&mut cursor, 4, &result, None),
            AdaptiveLoopDisposition::ReplanOnce
        );
        assert_eq!(
            observe(&mut cursor, 4, &result, None),
            AdaptiveLoopDisposition::CommitTerminalResult
        );
        assert_eq!(cursor.no_gain_count(), MAX_ADAPTIVE_NO_GAIN_COUNT);
        assert!(cursor.replan_emitted());
    }

    #[test]
    fn excluded_model_text_and_raw_result_fields_do_not_create_progress() {
        let mut cursor = AdaptiveLoopCursor::default();
        let first = result(ToolOutcomeStatus::Succeeded, true, &[("revision", "1")]);
        assert_eq!(
            observe(&mut cursor, 0, &first, None),
            AdaptiveLoopDisposition::Continue
        );

        let mut changed = first.clone();
        changed.output = "different raw output".to_string();
        changed.structured_output_json = Some("different raw input-shaped data".to_string());
        changed
            .metadata
            .insert("input_json".to_string(), "secret".to_string());
        let observation = changed.model_observation.as_mut().unwrap();
        observation.summary = "different summary".to_string();
        observation.evidence = "different evidence".to_string();
        observation.next_action = Some("different next action".to_string());

        assert_eq!(
            observe(&mut cursor, 0, &changed, None),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(cursor.no_gain_count(), 1);
    }

    #[test]
    fn included_typed_fields_reset_no_gain() {
        let mut cursor = AdaptiveLoopCursor::default();
        let base = result(ToolOutcomeStatus::Succeeded, true, &[("revision", "1")]);
        observe(&mut cursor, 0, &base, None);
        observe(&mut cursor, 0, &base, None);
        assert_eq!(cursor.no_gain_count(), 1);

        let facts_changed = result(ToolOutcomeStatus::Succeeded, true, &[("revision", "2")]);
        observe(&mut cursor, 0, &facts_changed, None);
        assert_eq!(cursor.no_gain_count(), 0);

        let mut failure_changed = facts_changed.clone();
        failure_changed.status = ToolOutcomeStatus::Failed;
        failure_changed.failure = Some(ToolFailure {
            code: "temporary".to_string(),
            message: "excluded failure message".to_string(),
            retryable: true,
        });
        observe(&mut cursor, 0, &failure_changed, None);
        assert_eq!(cursor.no_gain_count(), 0);

        failure_changed.failure.as_mut().unwrap().retryable = false;
        observe(&mut cursor, 0, &failure_changed, None);
        assert_eq!(cursor.no_gain_count(), 0);
    }

    #[test]
    fn changing_exact_action_input_fingerprint_resets_no_gain() {
        let mut cursor = AdaptiveLoopCursor::default();
        let stable = result(ToolOutcomeStatus::Succeeded, true, &[("revision", "1")]);
        assert_eq!(
            cursor.observe_tool_result(
                0,
                "workspace.read",
                INPUT_A,
                &stable,
                Some(&ToolEffectSemantics::NonIdempotent),
                None,
            ),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(
            cursor.observe_tool_result(
                0,
                "workspace.read",
                INPUT_A,
                &stable,
                Some(&ToolEffectSemantics::NonIdempotent),
                None,
            ),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(cursor.no_gain_count(), 1);

        assert_eq!(
            cursor.observe_tool_result(
                0,
                "workspace.read",
                INPUT_B,
                &stable,
                Some(&ToolEffectSemantics::NonIdempotent),
                None,
            ),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(cursor.no_gain_count(), 0);
        assert!(!cursor.replan_emitted());
    }

    #[test]
    fn incomplete_evidence_never_counts_as_stagnation() {
        let mut cursor = AdaptiveLoopCursor::default();
        let pending = result(ToolOutcomeStatus::Succeeded, false, &[("state", "running")]);
        for _ in 0..12 {
            assert_eq!(
                observe(&mut cursor, 0, &pending, None),
                AdaptiveLoopDisposition::Continue
            );
        }
        assert_eq!(cursor.no_gain_count(), 0);
        assert!(!cursor.replan_emitted());
    }

    #[test]
    fn steer_and_goal_delta_each_reset_the_epoch_cursor() {
        let mut cursor = AdaptiveLoopCursor::default();
        let stable = result(ToolOutcomeStatus::Succeeded, true, &[("revision", "1")]);
        for _ in 0..3 {
            observe(&mut cursor, 0, &stable, None);
        }
        assert!(cursor.replan_emitted());

        assert_eq!(
            observe(&mut cursor, 1, &stable, None),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(cursor.steer_epoch(), 1);
        assert_eq!(cursor.no_gain_count(), 0);
        assert!(!cursor.replan_emitted());

        for _ in 0..2 {
            observe(&mut cursor, 1, &stable, None);
        }
        let delta: AgentGoalDelta = serde_json::from_str(
            r#"{"schema":"cindx.agent.goal-delta.v1","kinds":["obligation_satisfied"],"receiptIds":[1],"fingerprint":"test"}"#,
        )
        .unwrap();
        assert_eq!(
            observe(&mut cursor, 1, &stable, Some(&delta)),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(cursor.no_gain_count(), 0);
        assert!(!cursor.replan_emitted());
    }

    #[test]
    fn untyped_or_wrong_schema_interruptions_clear_stale_escalation() {
        let typed = result(ToolOutcomeStatus::Succeeded, true, &[("revision", "1")]);
        let mut legacy = typed.clone();
        legacy.model_observation = None;
        let mut wrong_schema = typed.clone();
        wrong_schema.model_observation.as_mut().unwrap().schema = "legacy".to_string();

        for interruption in [&legacy, &wrong_schema] {
            let mut cursor = AdaptiveLoopCursor::for_steer_epoch(9);
            assert_eq!(
                observe(&mut cursor, 9, &typed, None),
                AdaptiveLoopDisposition::Continue
            );
            assert_eq!(
                observe(&mut cursor, 9, &typed, None),
                AdaptiveLoopDisposition::Continue
            );
            assert_eq!(
                observe(&mut cursor, 9, &typed, None),
                AdaptiveLoopDisposition::ReplanOnce
            );
            assert!(cursor.replan_emitted());

            assert_eq!(
                observe(&mut cursor, 9, interruption, None),
                AdaptiveLoopDisposition::Continue
            );
            assert_eq!(cursor.no_gain_count(), 0);
            assert!(!cursor.replan_emitted());
            assert_eq!(cursor.disposition(), AdaptiveLoopDisposition::Continue);

            // The next known observation is a fresh baseline, not a stale
            // third repeat that could commit a terminal result.
            assert_eq!(
                observe(&mut cursor, 9, &typed, None),
                AdaptiveLoopDisposition::Continue
            );
            assert_eq!(cursor.no_gain_count(), 0);
        }
    }

    #[test]
    fn cursor_serialization_is_fixed_size_and_backward_defaultable() {
        let mut cursor = AdaptiveLoopCursor::default();
        let huge = "x".repeat(1_000_000);
        let observation = result(
            ToolOutcomeStatus::Succeeded,
            true,
            &[("huge", huge.as_str())],
        );
        observe(&mut cursor, 0, &observation, None);
        let encoded = serde_json::to_vec(&cursor).unwrap();
        assert!(encoded.len() < 512);
        assert_eq!(
            serde_json::from_slice::<AdaptiveLoopCursor>(&encoded).unwrap(),
            cursor
        );

        let legacy = serde_json::from_str::<AdaptiveLoopCursor>("{}").unwrap();
        assert_eq!(legacy, AdaptiveLoopCursor::default());

        let capped = serde_json::from_str::<AdaptiveLoopCursor>(
            r#"{"steerEpoch":0,"noGainCount":255,"replanEmitted":false}"#,
        )
        .unwrap();
        assert_eq!(capped.no_gain_count(), MAX_ADAPTIVE_NO_GAIN_COUNT);
    }
}
