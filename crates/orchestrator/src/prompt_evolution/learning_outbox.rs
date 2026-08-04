use super::{PromptAutoTransferIntent, PromptProDistillationIntent};
use crate::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PROMPT_LEARNING_OUTBOX_SCHEMA: &str = "cindx.prompt-learning-outbox.v1";
pub const PROMPT_LEARNING_OUTBOX_VERSION: u32 = 2;
pub const PROMPT_LEARNING_OUTBOX_MAX_PENDING: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PendingIntentEnvelope {
    project_id: String,
    intent_id: String,
    sequence: u64,
    payload_sha256: String,
    payload: String,
}

impl PendingIntentEnvelope {
    fn new(
        project_id: &str,
        intent_id: &str,
        sequence: u64,
        payload: String,
    ) -> Result<Self, String> {
        let project_id = project_id.trim();
        let intent_id = intent_id.trim();
        if project_id.is_empty()
            || intent_id.is_empty()
            || sequence == 0
            || payload.trim().is_empty()
        {
            return Err("prompt learning outbox intent identity is incomplete".to_string());
        }
        Ok(Self {
            project_id: project_id.to_string(),
            intent_id: intent_id.to_string(),
            sequence,
            payload_sha256: sha256_hex(payload.as_bytes()),
            payload,
        })
    }

    fn is_valid(&self) -> bool {
        !self.project_id.trim().is_empty()
            && !self.intent_id.trim().is_empty()
            && self.sequence > 0
            && !self.payload.trim().is_empty()
            && self.payload_sha256 == sha256_hex(self.payload.as_bytes())
    }

    fn exact_payload(&self, candidate: &Self) -> bool {
        self.project_id == candidate.project_id
            && self.intent_id == candidate.intent_id
            && self.payload_sha256 == candidate.payload_sha256
            && self.payload == candidate.payload
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptLearningOutboxLoadPath {
    #[default]
    FullReplay,
    FastTailDelta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptLearningDispatchRecovery {
    EnqueueThenMark,
    MarkExistingRequest,
}

pub fn prompt_learning_dispatch_recovery(request_exists: bool) -> PromptLearningDispatchRecovery {
    if request_exists {
        PromptLearningDispatchRecovery::MarkExistingRequest
    } else {
        PromptLearningDispatchRecovery::EnqueueThenMark
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptLearningOutboxProjection {
    schema: String,
    version: u32,
    revision: u64,
    event_count: u64,
    overflowed: bool,
    pending_auto_transfer: BTreeMap<String, PendingIntentEnvelope>,
    pending_pro_distillation: BTreeMap<String, PendingIntentEnvelope>,
    #[serde(skip, default)]
    load_path: PromptLearningOutboxLoadPath,
    #[serde(skip, default)]
    replaying_full_history: bool,
    #[serde(skip, default)]
    overflow_recovery_required: bool,
}

impl PromptLearningOutboxProjection {
    pub fn empty() -> Self {
        Self {
            schema: PROMPT_LEARNING_OUTBOX_SCHEMA.to_string(),
            version: PROMPT_LEARNING_OUTBOX_VERSION,
            revision: 0,
            event_count: 0,
            overflowed: false,
            pending_auto_transfer: BTreeMap::new(),
            pending_pro_distillation: BTreeMap::new(),
            load_path: PromptLearningOutboxLoadPath::FullReplay,
            replaying_full_history: false,
            overflow_recovery_required: false,
        }
    }

    pub fn begin_full_replay() -> Self {
        let mut projection = Self::empty();
        projection.replaying_full_history = true;
        projection
    }

    pub fn finish_full_replay(&mut self) {
        let mut ranked = self
            .pending_auto_transfer
            .iter()
            .map(|(project_id, envelope)| (envelope.sequence, false, project_id.clone()))
            .chain(
                self.pending_pro_distillation
                    .iter()
                    .map(|(project_id, envelope)| (envelope.sequence, true, project_id.clone())),
            )
            .collect::<Vec<_>>();
        ranked.sort();
        self.overflowed = ranked.len() > PROMPT_LEARNING_OUTBOX_MAX_PENDING;
        if self.overflowed {
            let retained = ranked
                .into_iter()
                .take(PROMPT_LEARNING_OUTBOX_MAX_PENDING)
                .map(|(_, pro_distillation, project_id)| (pro_distillation, project_id))
                .collect::<BTreeSet<_>>();
            self.pending_auto_transfer
                .retain(|project_id, _| retained.contains(&(false, project_id.clone())));
            self.pending_pro_distillation
                .retain(|project_id, _| retained.contains(&(true, project_id.clone())));
        }
        self.load_path = PromptLearningOutboxLoadPath::FullReplay;
        self.replaying_full_history = false;
        self.overflow_recovery_required = false;
    }

    pub fn is_intrinsically_valid(&self) -> bool {
        self.schema == PROMPT_LEARNING_OUTBOX_SCHEMA
            && self.version == PROMPT_LEARNING_OUTBOX_VERSION
            && self.pending_len() <= PROMPT_LEARNING_OUTBOX_MAX_PENDING
            && (!self.overflowed || self.pending_len() == PROMPT_LEARNING_OUTBOX_MAX_PENDING)
            && !self.replaying_full_history
            && !self.overflow_recovery_required
            && self
                .pending_auto_transfer
                .iter()
                .chain(self.pending_pro_distillation.iter())
                .all(|(project_id, envelope)| {
                    project_id == &envelope.project_id
                        && envelope.sequence <= self.revision
                        && envelope.is_valid()
                })
            && self.pending_intent_ids_are_unique()
            && self.pending_payloads_are_valid()
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn event_count(&self) -> u64 {
        self.event_count
    }

    pub fn set_cursor(&mut self, revision: u64, event_count: u64) {
        self.revision = revision;
        self.event_count = event_count;
    }

    pub fn set_fast_tail_cursor(&mut self, revision: u64, event_count: u64) {
        self.set_cursor(revision, event_count);
        self.load_path = PromptLearningOutboxLoadPath::FastTailDelta;
    }

    pub const fn load_path(&self) -> PromptLearningOutboxLoadPath {
        self.load_path
    }

    pub fn used_fast_revision_path(&self) -> bool {
        self.load_path == PromptLearningOutboxLoadPath::FastTailDelta
    }

    pub const fn overflowed(&self) -> bool {
        self.overflowed
    }

    pub const fn overflow_recovery_required(&self) -> bool {
        self.overflow_recovery_required
    }

    pub fn insert_auto_transfer_intent(
        &mut self,
        sequence: u64,
        intent: &PromptAutoTransferIntent,
    ) -> Result<(), String> {
        if !intent.validate() {
            return Err("prompt Auto transfer intent is invalid".to_string());
        }
        let project_id = intent
            .project_id()
            .ok_or_else(|| "prompt Auto transfer project is missing".to_string())?;
        self.insert_auto_transfer(project_id, intent.intent_id(), sequence, intent.to_json()?)
    }

    pub fn insert_pro_distillation_intent(
        &mut self,
        sequence: u64,
        intent: &PromptProDistillationIntent,
    ) -> Result<(), String> {
        if !intent.validate() {
            return Err("prompt Pro distillation intent is invalid".to_string());
        }
        let project_id = intent
            .project_id()
            .ok_or_else(|| "prompt Pro distillation project is missing".to_string())?;
        self.insert_pro_distillation(project_id, intent.intent_id(), sequence, intent.to_json()?)
    }

    fn insert_auto_transfer(
        &mut self,
        project_id: &str,
        intent_id: &str,
        sequence: u64,
        payload: String,
    ) -> Result<(), String> {
        self.insert_pending(true, project_id, intent_id, sequence, payload)
    }

    fn insert_pro_distillation(
        &mut self,
        project_id: &str,
        intent_id: &str,
        sequence: u64,
        payload: String,
    ) -> Result<(), String> {
        self.insert_pending(false, project_id, intent_id, sequence, payload)
    }

    pub fn remove_auto_transfer(
        &mut self,
        project_id: &str,
        intent_id: &str,
    ) -> Result<(), String> {
        self.remove_pending(true, project_id, intent_id)
    }

    pub fn remove_pro_distillation(
        &mut self,
        project_id: &str,
        intent_id: &str,
    ) -> Result<(), String> {
        self.remove_pending(false, project_id, intent_id)
    }

    pub fn pending_auto_transfer(&self) -> impl Iterator<Item = (&str, &str, &str)> + '_ {
        let mut pending = self.pending_auto_transfer.values().collect::<Vec<_>>();
        pending.sort_by(|left, right| {
            left.sequence
                .cmp(&right.sequence)
                .then_with(|| left.project_id.cmp(&right.project_id))
                .then_with(|| left.intent_id.cmp(&right.intent_id))
        });
        pending.into_iter().map(|envelope| {
            (
                envelope.project_id.as_str(),
                envelope.intent_id.as_str(),
                envelope.payload.as_str(),
            )
        })
    }

    pub fn pending_pro_distillation(&self) -> impl Iterator<Item = (&str, &str, &str)> + '_ {
        let mut pending = self.pending_pro_distillation.values().collect::<Vec<_>>();
        pending.sort_by(|left, right| {
            left.sequence
                .cmp(&right.sequence)
                .then_with(|| left.project_id.cmp(&right.project_id))
                .then_with(|| left.intent_id.cmp(&right.intent_id))
        });
        pending.into_iter().map(|envelope| {
            (
                envelope.project_id.as_str(),
                envelope.intent_id.as_str(),
                envelope.payload.as_str(),
            )
        })
    }

    pub fn contains_auto_transfer(&self, project_id: &str, intent_id: &str) -> bool {
        self.pending_auto_transfer
            .get(project_id)
            .is_some_and(|pending| pending.intent_id == intent_id)
    }

    pub fn contains_pro_distillation(&self, project_id: &str, intent_id: &str) -> bool {
        self.pending_pro_distillation
            .get(project_id)
            .is_some_and(|pending| pending.intent_id == intent_id)
    }

    pub fn pending_len(&self) -> usize {
        self.pending_auto_transfer
            .len()
            .saturating_add(self.pending_pro_distillation.len())
    }

    fn insert_pending(
        &mut self,
        auto_transfer: bool,
        project_id: &str,
        intent_id: &str,
        sequence: u64,
        payload: String,
    ) -> Result<(), String> {
        let envelope = PendingIntentEnvelope::new(project_id, intent_id, sequence, payload)?;
        let (target, other) = if auto_transfer {
            (
                &mut self.pending_auto_transfer,
                &self.pending_pro_distillation,
            )
        } else {
            (
                &mut self.pending_pro_distillation,
                &self.pending_auto_transfer,
            )
        };
        if target.values().chain(other.values()).any(|existing| {
            existing.intent_id == envelope.intent_id && existing.project_id != envelope.project_id
        }) {
            return Err(format!(
                "prompt learning outbox intent {} changes project scope",
                envelope.intent_id
            ));
        }
        if other
            .values()
            .any(|existing| existing.intent_id == envelope.intent_id)
        {
            return Err(format!(
                "prompt learning outbox intent {} changes learning track",
                envelope.intent_id
            ));
        }
        if let Some(existing) = target.get(&envelope.project_id) {
            if existing.intent_id == envelope.intent_id {
                if existing.exact_payload(&envelope) {
                    return Ok(());
                }
                return Err(format!(
                    "prompt learning outbox intent {} has conflicting payloads",
                    envelope.intent_id
                ));
            }
            target.insert(envelope.project_id.clone(), envelope);
            if self.overflowed && !self.replaying_full_history {
                self.overflow_recovery_required = true;
            }
            return Ok(());
        }
        if !self.replaying_full_history
            && target.len().saturating_add(other.len()) >= PROMPT_LEARNING_OUTBOX_MAX_PENDING
        {
            self.overflowed = true;
            return Ok(());
        }
        target.insert(envelope.project_id.clone(), envelope);
        Ok(())
    }

    fn remove_pending(
        &mut self,
        auto_transfer: bool,
        project_id: &str,
        intent_id: &str,
    ) -> Result<(), String> {
        let project_id = project_id.trim();
        let intent_id = intent_id.trim();
        if project_id.is_empty() || intent_id.is_empty() {
            return Err("prompt learning outbox dispatch scope is incomplete".to_string());
        }
        let target = if auto_transfer {
            &mut self.pending_auto_transfer
        } else {
            &mut self.pending_pro_distillation
        };
        if target
            .values()
            .any(|existing| existing.intent_id == intent_id && existing.project_id != project_id)
        {
            return Err(format!(
                "prompt learning outbox dispatch {intent_id} has mismatched project scope"
            ));
        }
        let remove = target
            .get(project_id)
            .is_some_and(|existing| existing.intent_id == intent_id);
        if remove {
            target.remove(project_id);
            if self.overflowed && !self.replaying_full_history {
                self.overflow_recovery_required = true;
            }
        }
        Ok(())
    }

    fn pending_intent_ids_are_unique(&self) -> bool {
        let mut ids = BTreeSet::new();
        self.pending_auto_transfer
            .values()
            .chain(self.pending_pro_distillation.values())
            .all(|envelope| ids.insert(envelope.intent_id.as_str()))
    }

    fn pending_payloads_are_valid(&self) -> bool {
        self.pending_auto_transfer.values().all(|envelope| {
            PromptAutoTransferIntent::decode_for_project(
                &envelope.project_id,
                &envelope.intent_id,
                &envelope.payload,
            )
            .is_ok()
        }) && self.pending_pro_distillation.values().all(|envelope| {
            PromptProDistillationIntent::decode_for_project(
                &envelope.project_id,
                &envelope.intent_id,
                &envelope.payload,
            )
            .is_ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Metadata, TaskId};

    fn auto_intent(project_id: &str, run_id: &str) -> PromptAutoTransferIntent {
        let context = [
            ("project_id".to_string(), project_id.to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        PromptAutoTransferIntent::from_context(&TaskId("task".to_string()), &context).unwrap()
    }

    #[test]
    fn v2_wire_shape_remains_compatible() {
        assert_eq!(
            serde_json::to_string(&PromptLearningOutboxProjection::empty()).unwrap(),
            r#"{"schema":"cindx.prompt-learning-outbox.v1","version":2,"revision":0,"event_count":0,"overflowed":false,"pending_auto_transfer":{},"pending_pro_distillation":{}}"#
        );

        let mut populated = PromptLearningOutboxProjection::begin_full_replay();
        populated
            .insert_auto_transfer("project", "intent", 2, "payload".to_string())
            .unwrap();
        populated.finish_full_replay();
        populated.set_cursor(3, 3);
        let golden = r#"{"schema":"cindx.prompt-learning-outbox.v1","version":2,"revision":3,"event_count":3,"overflowed":false,"pending_auto_transfer":{"project":{"project_id":"project","intent_id":"intent","sequence":2,"payload_sha256":"239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5","payload":"payload"}},"pending_pro_distillation":{}}"#;
        assert_eq!(serde_json::to_string(&populated).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<PromptLearningOutboxProjection>(golden).unwrap(),
            populated
        );
    }

    #[test]
    fn exact_replay_is_idempotent_and_identity_changes_fail_closed() {
        let mut projection = PromptLearningOutboxProjection::begin_full_replay();
        projection
            .insert_auto_transfer("project", "intent", 1, "payload".to_string())
            .unwrap();
        projection
            .insert_auto_transfer("project", "intent", 2, "payload".to_string())
            .unwrap();
        assert!(projection
            .insert_auto_transfer("project", "intent", 3, "different".to_string())
            .unwrap_err()
            .contains("conflicting payloads"));
        assert!(projection
            .insert_auto_transfer("other", "intent", 4, "payload".to_string())
            .unwrap_err()
            .contains("changes project scope"));
        assert!(projection
            .insert_pro_distillation("project", "intent", 4, "payload".to_string())
            .unwrap_err()
            .contains("changes learning track"));
        assert_eq!(projection.pending_len(), 1);
    }

    #[test]
    fn public_auto_insertion_accepts_only_a_typed_valid_intent() {
        let intent = auto_intent("project", "run");
        let mut projection = PromptLearningOutboxProjection::begin_full_replay();
        projection.insert_auto_transfer_intent(1, &intent).unwrap();
        projection.finish_full_replay();
        projection.set_cursor(1, 1);
        let payload = intent.to_json().unwrap();

        assert_eq!(
            projection.pending_auto_transfer().next(),
            Some(("project", intent.intent_id(), payload.as_str()))
        );
        assert!(projection.is_intrinsically_valid());
    }

    #[test]
    fn pending_work_is_fifo_instead_of_project_lexicographic_order() {
        let mut projection = PromptLearningOutboxProjection::begin_full_replay();
        projection
            .insert_auto_transfer("z-project", "auto-oldest", 1, "auto-z".to_string())
            .unwrap();
        projection
            .insert_auto_transfer("a-project", "auto-newest", 4, "auto-a".to_string())
            .unwrap();
        projection
            .insert_pro_distillation("z-pro", "pro-oldest", 2, "pro-z".to_string())
            .unwrap();
        projection
            .insert_pro_distillation("a-pro", "pro-newest", 3, "pro-a".to_string())
            .unwrap();
        projection.finish_full_replay();
        projection.set_cursor(4, 4);

        assert_eq!(
            projection.pending_auto_transfer().collect::<Vec<_>>(),
            vec![
                ("z-project", "auto-oldest", "auto-z"),
                ("a-project", "auto-newest", "auto-a"),
            ]
        );
        assert_eq!(
            projection.pending_pro_distillation().collect::<Vec<_>>(),
            vec![
                ("z-pro", "pro-oldest", "pro-z"),
                ("a-pro", "pro-newest", "pro-a"),
            ]
        );
    }

    #[test]
    fn fifo_ties_are_deterministic() {
        let mut projection = PromptLearningOutboxProjection::begin_full_replay();
        projection
            .insert_auto_transfer("z-project", "z-intent", 1, "z".to_string())
            .unwrap();
        projection
            .insert_auto_transfer("a-project", "a-intent", 1, "a".to_string())
            .unwrap();
        projection.finish_full_replay();
        projection.set_cursor(1, 2);

        assert_eq!(
            projection.pending_auto_transfer().collect::<Vec<_>>(),
            vec![
                ("a-project", "a-intent", "a"),
                ("z-project", "z-intent", "z"),
            ]
        );
    }

    #[test]
    fn overflow_window_refills_after_capacity_is_released() {
        let mut projection = PromptLearningOutboxProjection::begin_full_replay();
        for index in 0..=PROMPT_LEARNING_OUTBOX_MAX_PENDING {
            projection
                .insert_auto_transfer(
                    &format!("project-{index:03}"),
                    &format!("intent-{index:03}"),
                    index as u64 + 1,
                    format!("payload-{index:03}"),
                )
                .unwrap();
        }
        projection.finish_full_replay();
        projection.set_cursor(PROMPT_LEARNING_OUTBOX_MAX_PENDING as u64 + 1, 257);
        assert!(projection.overflowed());
        assert_eq!(projection.pending_len(), PROMPT_LEARNING_OUTBOX_MAX_PENDING);
        assert!(projection.contains_auto_transfer("project-000", "intent-000"));
        assert!(!projection.contains_auto_transfer("project-256", "intent-256"));

        projection
            .remove_auto_transfer("project-000", "intent-000")
            .unwrap();
        assert!(projection.overflow_recovery_required());

        let mut refilled = PromptLearningOutboxProjection::begin_full_replay();
        for index in 1..=PROMPT_LEARNING_OUTBOX_MAX_PENDING {
            refilled
                .insert_auto_transfer(
                    &format!("project-{index:03}"),
                    &format!("intent-{index:03}"),
                    index as u64 + 1,
                    format!("payload-{index:03}"),
                )
                .unwrap();
        }
        refilled.finish_full_replay();
        refilled.set_cursor(PROMPT_LEARNING_OUTBOX_MAX_PENDING as u64 + 2, 258);
        assert!(!refilled.overflowed());
        assert_eq!(refilled.pending_len(), PROMPT_LEARNING_OUTBOX_MAX_PENDING);
        assert!(!refilled.contains_auto_transfer("project-000", "intent-000"));
        assert!(refilled.contains_auto_transfer("project-256", "intent-256"));
    }

    #[test]
    fn overflowed_window_requires_replay_when_a_retained_project_is_replaced() {
        let mut projection = PromptLearningOutboxProjection::begin_full_replay();
        for index in 0..=PROMPT_LEARNING_OUTBOX_MAX_PENDING {
            projection
                .insert_auto_transfer(
                    &format!("project-{index:03}"),
                    &format!("intent-{index:03}"),
                    index as u64 + 1,
                    format!("payload-{index:03}"),
                )
                .unwrap();
        }
        projection.finish_full_replay();
        projection.set_cursor(PROMPT_LEARNING_OUTBOX_MAX_PENDING as u64 + 1, 257);
        assert!(projection.overflowed());

        projection
            .insert_auto_transfer(
                "project-000",
                "replacement",
                PROMPT_LEARNING_OUTBOX_MAX_PENDING as u64 + 2,
                "replacement-payload".to_string(),
            )
            .unwrap();

        assert!(projection.overflow_recovery_required());
    }

    #[test]
    fn existing_request_uses_mark_only_recovery() {
        assert_eq!(
            prompt_learning_dispatch_recovery(true),
            PromptLearningDispatchRecovery::MarkExistingRequest
        );
        assert_eq!(
            prompt_learning_dispatch_recovery(false),
            PromptLearningDispatchRecovery::EnqueueThenMark
        );
    }

    #[test]
    fn corrupted_payload_digest_is_intrinsically_invalid() {
        let mut projection = PromptLearningOutboxProjection::begin_full_replay();
        projection
            .insert_auto_transfer("project", "intent", 1, "canonical".to_string())
            .unwrap();
        projection.finish_full_replay();
        projection.set_cursor(1, 1);
        let mut value = serde_json::to_value(&projection).unwrap();
        value["pending_auto_transfer"]["project"]["payload"] =
            serde_json::Value::String("corrupt".to_string());
        let corrupt: PromptLearningOutboxProjection = serde_json::from_value(value).unwrap();
        assert!(!corrupt.is_intrinsically_valid());
    }

    #[test]
    fn cursor_and_fast_tail_path_are_explicit_non_persistent_state() {
        let mut projection = PromptLearningOutboxProjection::empty();
        projection.set_fast_tail_cursor(7, 5);
        assert_eq!(projection.revision(), 7);
        assert_eq!(projection.event_count(), 5);
        assert_eq!(
            projection.load_path(),
            PromptLearningOutboxLoadPath::FastTailDelta
        );
        assert!(projection.used_fast_revision_path());

        let restored: PromptLearningOutboxProjection =
            serde_json::from_str(&serde_json::to_string(&projection).unwrap()).unwrap();
        assert_eq!(
            restored.load_path(),
            PromptLearningOutboxLoadPath::FullReplay
        );
        assert!(!restored.used_fast_revision_path());
    }
}
