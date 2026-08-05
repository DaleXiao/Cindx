use crate::{
    ActionableSideInformation, AgentEvaluationCheck, AgentEvaluationEvidenceSource,
    AgentEvaluationReflectionPacket, AgentEvaluationTraceStep, AgentEvaluationVerifierOutcome,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const PROMPT_FAILURE_CURRICULUM_SCHEMA_V1: &str = "cindx.prompt-failure-curriculum.v1";
const PROMPT_FAILURE_CURRICULUM_MAX_BYTES: usize = 2 * 1024;
const PROMPT_FAILURE_CODE_MAX_BYTES: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptFailureCurriculumKind {
    Timeout,
    Denial,
    NoProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptFailureCurriculumRole {
    FailureSeed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptFailureDenialKind {
    UserPermission,
    RuntimePolicy,
    CapabilityUnavailable,
    RepeatedAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptFailureCurriculumReceiptV1 {
    pub schema: String,
    pub role: PromptFailureCurriculumRole,
    pub kind: PromptFailureCurriculumKind,
    pub project_sha256: String,
    pub run_sha256: String,
    pub profile_sha256: String,
    pub policy_sha256: String,
    pub task_class_sha256: String,
    pub steer_epoch: u64,
    pub contract_epoch: u64,
    pub source_event_sequence: u64,
    pub source_evidence_sha256: String,
    pub failure_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial_kind: Option<PromptFailureDenialKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_fingerprint: Option<String>,
}

pub struct PromptFailureCurriculumInput<'a> {
    pub kind: PromptFailureCurriculumKind,
    pub project_id: &'a str,
    pub run_id: &'a str,
    pub profile_id: &'a str,
    pub policy: &'a str,
    pub task_class: &'a str,
    pub steer_epoch: u64,
    pub contract_epoch: u64,
    pub source_event_sequence: u64,
    pub source_evidence_sha256: &'a str,
    pub failure_code: &'a str,
    pub denial_kind: Option<PromptFailureDenialKind>,
    pub tool_name: Option<&'a str>,
    pub input_fingerprint: Option<&'a str>,
}

impl PromptFailureCurriculumReceiptV1 {
    pub fn new(input: PromptFailureCurriculumInput<'_>) -> Result<Self, String> {
        let receipt = Self {
            schema: PROMPT_FAILURE_CURRICULUM_SCHEMA_V1.to_string(),
            role: PromptFailureCurriculumRole::FailureSeed,
            kind: input.kind,
            project_sha256: failure_sha256(input.project_id.trim().as_bytes()),
            run_sha256: failure_sha256(input.run_id.trim().as_bytes()),
            profile_sha256: failure_sha256(input.profile_id.trim().as_bytes()),
            policy_sha256: failure_sha256(input.policy.trim().as_bytes()),
            task_class_sha256: failure_sha256(input.task_class.trim().as_bytes()),
            steer_epoch: input.steer_epoch,
            contract_epoch: input.contract_epoch,
            source_event_sequence: input.source_event_sequence,
            source_evidence_sha256: input.source_evidence_sha256.trim().to_ascii_lowercase(),
            failure_code: input.failure_code.trim().to_ascii_lowercase(),
            denial_kind: input.denial_kind,
            tool_sha256: input
                .tool_name
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
                .map(|tool| failure_sha256(tool.as_bytes())),
            input_fingerprint: input
                .input_fingerprint
                .map(str::trim)
                .filter(|fingerprint| !fingerprint.is_empty())
                .map(str::to_ascii_lowercase),
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_FAILURE_CURRICULUM_SCHEMA_V1
            || self.role != PromptFailureCurriculumRole::FailureSeed
            || self.contract_epoch != self.steer_epoch
            || self.source_event_sequence == 0
            || [
                &self.project_sha256,
                &self.run_sha256,
                &self.profile_sha256,
                &self.policy_sha256,
                &self.task_class_sha256,
                &self.source_evidence_sha256,
            ]
            .into_iter()
            .any(|digest| !is_sha256(digest))
            || self.failure_code.is_empty()
            || self.failure_code.len() > PROMPT_FAILURE_CODE_MAX_BYTES
            || !self.failure_code.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'-' | b'.')
            })
        {
            return Err("prompt failure curriculum receipt is malformed".to_string());
        }
        let denial_fields_are_valid = self.denial_kind.is_some()
            && self
                .tool_sha256
                .as_ref()
                .is_some_and(|digest| is_sha256(digest))
            && self
                .input_fingerprint
                .as_ref()
                .is_some_and(|fingerprint| is_sha256(fingerprint));
        if (self.kind == PromptFailureCurriculumKind::Denial) != denial_fields_are_valid
            || (self.kind != PromptFailureCurriculumKind::Denial
                && (self.denial_kind.is_some()
                    || self.tool_sha256.is_some()
                    || self.input_fingerprint.is_some()))
            || serde_json::to_vec(self)
                .map(|encoded| encoded.len() > PROMPT_FAILURE_CURRICULUM_MAX_BYTES)
                .unwrap_or(true)
        {
            return Err("prompt failure curriculum attribution is invalid".to_string());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        let mut identity = self.clone();
        identity.source_event_sequence = 1;
        serde_json::to_vec(&identity)
            .map(|encoded| failure_sha256(&encoded))
            .map_err(|error| format!("failure curriculum serialization failed: {error}"))
    }

    pub fn matches_profile(&self, profile_id: &str) -> bool {
        self.validate().is_ok()
            && self.profile_sha256 == failure_sha256(profile_id.trim().as_bytes())
    }

    pub fn matches_run(&self, run_id: &str) -> bool {
        self.validate().is_ok() && self.run_sha256 == failure_sha256(run_id.trim().as_bytes())
    }

    pub fn matches_project(&self, project_id: &str) -> bool {
        self.validate().is_ok()
            && self.project_sha256 == failure_sha256(project_id.trim().as_bytes())
    }

    fn reflection_packet(
        &self,
        profile_id: &str,
    ) -> Result<AgentEvaluationReflectionPacket, String> {
        if !self.matches_profile(profile_id) {
            return Err("failure curriculum profile lineage is invalid".to_string());
        }
        let digest = self.digest()?;
        let (objective, failure, change) = match self.kind {
            PromptFailureCurriculumKind::Timeout => (
                "Finish essential work inside the existing time budget.",
                "The strategy exhausted its trusted time boundary before a terminal result.",
                "Reduce nonessential branches, preserve verified work, and commit or stop earlier without requesting a wider budget.",
            ),
            PromptFailureCurriculumKind::Denial => (
                "Respect an authoritative action denial without widening authority.",
                "The strategy depended on an action the runtime denied or could not expose.",
                "Do not repeat or disguise the denied action. Use a distinct already-authorized path only when it still satisfies the objective; otherwise finalize with the blocker.",
            ),
            PromptFailureCurriculumKind::NoProgress => (
                "Produce measurable Goal Delta or stop the stalled strategy.",
                "The strategy repeated activity without new trusted progress.",
                "Change the approach after the first repeated failure and stop when no authorized path can add Goal Delta.",
            ),
        };
        Ok(AgentEvaluationReflectionPacket {
            suite_id: PROMPT_FAILURE_CURRICULUM_SCHEMA_V1.to_string(),
            suite_version: 1,
            case_id: digest.clone(),
            category: format!("failure_curriculum:{:?}", self.kind).to_ascii_lowercase(),
            run_id: self.run_sha256.clone(),
            seed: self.steer_epoch,
            candidate_id: profile_id.to_string(),
            candidate_fingerprint: self.profile_sha256.clone(),
            model_fingerprints: BTreeMap::new(),
            input: objective.to_string(),
            steps: vec![AgentEvaluationTraceStep {
                step_id: "runtime_failure_observation".to_string(),
                role: "failure_observer".to_string(),
                model: "runtime_contract".to_string(),
                prompt: "Classify the trusted strategy failure without replaying user data."
                    .to_string(),
                output: failure.to_string(),
                tool_calls: Vec::new(),
                errors: vec![format!("failure_code={}", self.failure_code)],
                latency_ms: 0,
                total_tokens: 0,
            }],
            final_output: "Negative curriculum seed only; never a successful teacher or promotion receipt."
                .to_string(),
            verifier: AgentEvaluationVerifierOutcome {
                source: AgentEvaluationEvidenceSource::Deterministic,
                passed: false,
                score: 0.0,
                checks: vec![AgentEvaluationCheck {
                    id: "trusted_strategy_outcome".to_string(),
                    passed: false,
                    detail: failure.to_string(),
                }],
            },
            actionable_feedback: ActionableSideInformation {
                summary: "Trusted negative strategy outcome; retain only as bounded contrastive evidence."
                    .to_string(),
                passed_constraints: Vec::new(),
                failed_constraints: vec![failure.to_string()],
                errors: vec![format!("failure_code={}", self.failure_code)],
                suggested_changes: vec![change.to_string()],
            },
        })
    }
}

pub fn prompt_failure_reflection_packets(
    receipts: &[PromptFailureCurriculumReceiptV1],
    profile_id: &str,
    limit: usize,
) -> Vec<AgentEvaluationReflectionPacket> {
    if limit == 0 {
        return Vec::new();
    }
    let mut candidates = receipts
        .iter()
        .filter(|receipt| receipt.matches_profile(profile_id))
        .filter_map(|receipt| {
            Some((
                receipt.kind,
                receipt.source_event_sequence,
                receipt.digest().ok()?,
                receipt.reflection_packet(profile_id).ok()?,
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.2.cmp(&right.2))
    });
    let mut selected = Vec::new();
    let mut selected_digests = BTreeSet::<String>::new();
    let mut selected_kinds = BTreeSet::new();
    for (kind, _, digest, packet) in &candidates {
        if selected_kinds.insert(*kind) && selected_digests.insert(digest.clone()) {
            selected.push(packet.clone());
            if selected.len() == limit {
                return selected;
            }
        }
    }
    for (_, _, digest, packet) in candidates {
        if selected_digests.insert(digest) {
            selected.push(packet);
            if selected.len() == limit {
                break;
            }
        }
    }
    selected
}

fn failure_sha256(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(
        kind: PromptFailureCurriculumKind,
        sequence: u64,
    ) -> PromptFailureCurriculumReceiptV1 {
        PromptFailureCurriculumReceiptV1::new(PromptFailureCurriculumInput {
            kind,
            project_id: "project-secret-name",
            run_id: "run-secret-name",
            profile_id: "pro-profile",
            policy: "pro",
            task_class: "coding",
            steer_epoch: 3,
            contract_epoch: 3,
            source_event_sequence: sequence,
            source_evidence_sha256: &"a".repeat(64),
            failure_code: match kind {
                PromptFailureCurriculumKind::Timeout => "deadline_exceeded",
                PromptFailureCurriculumKind::Denial => "user_permission_denied",
                PromptFailureCurriculumKind::NoProgress => "no_progress",
            },
            denial_kind: (kind == PromptFailureCurriculumKind::Denial)
                .then_some(PromptFailureDenialKind::UserPermission),
            tool_name: (kind == PromptFailureCurriculumKind::Denial).then_some("shell.run"),
            input_fingerprint: (kind == PromptFailureCurriculumKind::Denial)
                .then_some("b".repeat(64))
                .as_deref(),
        })
        .unwrap()
    }

    #[test]
    fn failure_receipts_are_negative_only_bounded_and_redacted() {
        let receipt = receipt(PromptFailureCurriculumKind::Denial, 7);
        let encoded = serde_json::to_string(&receipt).unwrap();
        assert!(encoded.len() <= PROMPT_FAILURE_CURRICULUM_MAX_BYTES);
        assert!(!encoded.contains("project-secret-name"));
        assert!(!encoded.contains("run-secret-name"));
        assert!(!encoded.contains("shell.run"));
        let packet = receipt.reflection_packet("pro-profile").unwrap();
        assert!(!packet.verifier.passed);
        assert!(packet.final_output.contains("never a successful teacher"));
    }

    #[test]
    fn failure_selector_is_deterministic_diverse_and_profile_scoped() {
        let receipts = vec![
            receipt(PromptFailureCurriculumKind::Denial, 4),
            receipt(PromptFailureCurriculumKind::Denial, 5),
            receipt(PromptFailureCurriculumKind::NoProgress, 3),
            receipt(PromptFailureCurriculumKind::Timeout, 2),
        ];
        let selected = prompt_failure_reflection_packets(&receipts, "pro-profile", 3);
        let reversed = prompt_failure_reflection_packets(
            &receipts.iter().cloned().rev().collect::<Vec<_>>(),
            "pro-profile",
            3,
        );
        assert_eq!(selected, reversed);
        assert_eq!(selected.len(), 3);
        assert!(prompt_failure_reflection_packets(&receipts, "other-profile", 3).is_empty());
    }
}
