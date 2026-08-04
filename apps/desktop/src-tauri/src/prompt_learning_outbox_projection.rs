use agent_core::Event;
use agent_storage::{SqliteStore, StoredReadModel};
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const PROMPT_LEARNING_OUTBOX_SCHEMA: &str = "cindx.prompt-learning-outbox.v1";
const PROMPT_LEARNING_OUTBOX_VERSION: u32 = 2;
const PROMPT_LEARNING_OUTBOX_KEY: &str = "global";
const PROMPT_LEARNING_OUTBOX_MAX_PENDING: usize = 256;

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
enum PromptLearningOutboxLoadPath {
    #[default]
    FullReplay,
    FastTailDelta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PromptLearningDispatchRecovery {
    EnqueueThenMark,
    MarkExistingRequest,
}

pub(crate) fn prompt_learning_dispatch_recovery(
    request_exists: bool,
) -> PromptLearningDispatchRecovery {
    if request_exists {
        PromptLearningDispatchRecovery::MarkExistingRequest
    } else {
        PromptLearningDispatchRecovery::EnqueueThenMark
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PromptLearningOutboxProjection {
    schema: String,
    version: u32,
    pub(crate) revision: u64,
    pub(crate) event_count: u64,
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
    fn empty() -> Self {
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

    fn is_intrinsically_valid(&self) -> bool {
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
    }

    pub(crate) fn insert_auto_transfer(
        &mut self,
        project_id: &str,
        intent_id: &str,
        sequence: u64,
        payload: String,
    ) -> Result<(), String> {
        self.insert_pending(true, project_id, intent_id, sequence, payload)
    }

    pub(crate) fn insert_pro_distillation(
        &mut self,
        project_id: &str,
        intent_id: &str,
        sequence: u64,
        payload: String,
    ) -> Result<(), String> {
        self.insert_pending(false, project_id, intent_id, sequence, payload)
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

    pub(crate) fn remove_auto_transfer(
        &mut self,
        project_id: &str,
        intent_id: &str,
    ) -> Result<(), String> {
        self.remove_pending(true, project_id, intent_id)
    }

    pub(crate) fn remove_pro_distillation(
        &mut self,
        project_id: &str,
        intent_id: &str,
    ) -> Result<(), String> {
        self.remove_pending(false, project_id, intent_id)
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

    pub(crate) fn pending_auto_transfer(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.pending_auto_transfer
            .iter()
            .map(|(project_id, envelope)| {
                (
                    project_id.as_str(),
                    envelope.intent_id.as_str(),
                    envelope.payload.as_str(),
                )
            })
    }

    pub(crate) fn pending_pro_distillation(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.pending_pro_distillation
            .iter()
            .map(|(project_id, envelope)| {
                (
                    project_id.as_str(),
                    envelope.intent_id.as_str(),
                    envelope.payload.as_str(),
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn used_fast_revision_path(&self) -> bool {
        self.load_path == PromptLearningOutboxLoadPath::FastTailDelta
    }

    fn pending_intent_ids_are_unique(&self) -> bool {
        let mut ids = BTreeSet::new();
        self.pending_auto_transfer
            .values()
            .chain(self.pending_pro_distillation.values())
            .all(|envelope| ids.insert(envelope.intent_id.as_str()))
    }

    fn begin_full_replay() -> Self {
        let mut projection = Self::empty();
        projection.replaying_full_history = true;
        projection
    }

    fn finish_full_replay(&mut self) {
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
        self.replaying_full_history = false;
        self.overflow_recovery_required = false;
    }

    fn pending_len(&self) -> usize {
        self.pending_auto_transfer
            .len()
            .saturating_add(self.pending_pro_distillation.len())
    }
}

fn delta_is_contiguous(events: &[Event], after_sequence: u64, latest_sequence: u64) -> bool {
    if after_sequence == latest_sequence {
        return events.is_empty();
    }
    let Some(first_sequence) = after_sequence.checked_add(1) else {
        return false;
    };
    events.first().map(|event| event.sequence) == Some(first_sequence)
        && events.last().map(|event| event.sequence) == Some(latest_sequence)
        && events.windows(2).all(|window| {
            window[0]
                .sequence
                .checked_add(1)
                .is_some_and(|next| next == window[1].sequence)
        })
}

fn replay_full_history(
    store: &mut SqliteStore,
    project_event: &mut impl FnMut(&mut PromptLearningOutboxProjection, &Event) -> Result<(), String>,
) -> Result<Option<PromptLearningOutboxProjection>, String> {
    let task_id = crate::runtime_values::phase16_task_id();
    let revision = store
        .event_revision(&task_id)
        .map_err(|error| error.to_string())?;
    let events = store
        .list_by_task_after(&task_id, 0)
        .map_err(|error| error.to_string())?;
    if events.len() as u64 != revision.event_count
        || !delta_is_contiguous(&events, 0, revision.latest_sequence)
    {
        return Ok(None);
    }
    let mut projection = PromptLearningOutboxProjection::begin_full_replay();
    for event in &events {
        project_event(&mut projection, event)?;
    }
    projection.finish_full_replay();
    projection.revision = revision.latest_sequence;
    projection.event_count = revision.event_count;
    projection.load_path = PromptLearningOutboxLoadPath::FullReplay;
    Ok(Some(projection))
}

pub(crate) fn load_prompt_learning_outbox_projection(
    store: &mut SqliteStore,
    mut project_event: impl FnMut(&mut PromptLearningOutboxProjection, &Event) -> Result<(), String>,
    validate_payloads: impl Fn(&PromptLearningOutboxProjection) -> bool,
) -> Result<PromptLearningOutboxProjection, String> {
    for publish_attempt in 0..2 {
        let task_id = crate::runtime_values::phase16_task_id();
        let latest_sequence = store
            .latest_sequence(&task_id)
            .map_err(|error| error.to_string())?;
        let observed = store
            .load_read_model(PROMPT_LEARNING_OUTBOX_SCHEMA, PROMPT_LEARNING_OUTBOX_KEY)
            .map_err(|error| error.to_string())?;
        let stored = observed.as_ref().and_then(|stored| {
            serde_json::from_str::<PromptLearningOutboxProjection>(&stored.payload)
                .ok()
                .filter(|projection| {
                    projection.is_intrinsically_valid()
                        && validate_payloads(projection)
                        && projection.revision == stored.revision
                        && projection.revision <= latest_sequence
                })
        });
        let mut changed = stored.is_none();
        let mut full_replay_required = stored.is_none();
        let mut projection = stored.unwrap_or_else(PromptLearningOutboxProjection::empty);
        if !full_replay_required {
            let delta = store
                .list_by_task_after(&task_id, projection.revision)
                .map_err(|error| error.to_string())?;
            if !delta_is_contiguous(&delta, projection.revision, latest_sequence) {
                full_replay_required = true;
            } else {
                changed |= !delta.is_empty();
                for event in &delta {
                    project_event(&mut projection, event)?;
                }
                if projection.overflow_recovery_required {
                    full_replay_required = true;
                } else {
                    projection.revision = latest_sequence;
                    projection.event_count =
                        projection.event_count.saturating_add(delta.len() as u64);
                    projection.load_path = PromptLearningOutboxLoadPath::FastTailDelta;
                }
            }
        }
        if full_replay_required {
            changed = true;
            let Some(rebuilt) = replay_full_history(store, &mut project_event)? else {
                if publish_attempt == 0 {
                    continue;
                }
                return Err("prompt learning outbox canonical replay changed twice".to_string());
            };
            projection = rebuilt;
        }
        if !projection.is_intrinsically_valid() || !validate_payloads(&projection) {
            return Err("prompt learning outbox projection is invalid".to_string());
        }
        if !changed {
            return Ok(projection);
        }
        if compare_exchange_prompt_learning_outbox_projection(
            store,
            observed.as_ref(),
            &projection,
        )? {
            return Ok(projection);
        }
        if publish_attempt == 1 {
            return Err("prompt learning outbox snapshot publication conflicted twice".to_string());
        }
    }
    unreachable!("prompt learning outbox publication attempts are bounded")
}

pub(crate) fn compare_exchange_prompt_learning_outbox_projection(
    store: &mut SqliteStore,
    expected: Option<&StoredReadModel>,
    projection: &PromptLearningOutboxProjection,
) -> Result<bool, String> {
    let latest_sequence = store
        .latest_sequence(&crate::runtime_values::phase16_task_id())
        .map_err(|error| error.to_string())?;
    if projection.revision != latest_sequence || !projection.is_intrinsically_valid() {
        return Ok(false);
    }
    let payload = serde_json::to_string(projection)
        .map_err(|error| format!("prompt learning outbox serialization failed: {error}"))?;
    store
        .compare_exchange_read_model(
            PROMPT_LEARNING_OUTBOX_SCHEMA,
            PROMPT_LEARNING_OUTBOX_KEY,
            expected,
            projection.revision,
            &payload,
        )
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind, Metadata};
    use agent_storage::EventStore;
    use std::cell::Cell;

    fn event(
        sequence: u64,
        summary: &str,
        project_id: Option<&str>,
        intent_id: Option<&str>,
        payload: Option<&str>,
    ) -> Event {
        let mut metadata = Metadata::new();
        if let Some(project_id) = project_id {
            metadata.insert("project_id".to_string(), project_id.to_string());
        }
        if let Some(intent_id) = intent_id {
            metadata.insert("intent_id".to_string(), intent_id.to_string());
        }
        if let Some(payload) = payload {
            metadata.insert("payload".to_string(), payload.to_string());
        }
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: crate::runtime_values::phase16_task_id(),
            sequence,
            timestamp_ms: sequence,
            kind: EventKind::TaskStatusChanged,
            summary: summary.to_string(),
            metadata,
        }
    }

    fn project_test_event(
        projection: &mut PromptLearningOutboxProjection,
        event: &Event,
    ) -> Result<(), String> {
        match event.summary.as_str() {
            "intent" => projection.insert_auto_transfer(
                event.metadata.get("project_id").unwrap(),
                event.metadata.get("intent_id").unwrap(),
                event.sequence,
                event.metadata.get("payload").unwrap().clone(),
            ),
            "dispatch" => projection.remove_auto_transfer(
                event.metadata.get("project_id").unwrap(),
                event.metadata.get("intent_id").unwrap(),
            ),
            _ => Ok(()),
        }
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
        assert_eq!(
            projection.pending_auto_transfer().next(),
            Some(("project", "intent", "payload"))
        );
        assert!(projection
            .insert_auto_transfer("project", "intent", 3, "ambiguous".to_string())
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
    fn overflow_window_refills_after_capacity_is_released() {
        let mut store = SqliteStore::in_memory().unwrap();
        for index in 0..=PROMPT_LEARNING_OUTBOX_MAX_PENDING {
            let sequence = index as u64 + 1;
            store
                .append(event(
                    sequence,
                    "intent",
                    Some(&format!("project-{index:03}")),
                    Some(&format!("intent-{index:03}")),
                    Some(&format!("payload-{index:03}")),
                ))
                .unwrap();
        }
        let first =
            load_prompt_learning_outbox_projection(&mut store, project_test_event, |_| true)
                .unwrap();
        assert!(first.overflowed);
        assert_eq!(first.pending_len(), PROMPT_LEARNING_OUTBOX_MAX_PENDING);
        assert!(first.pending_auto_transfer.contains_key("project-000"));
        assert!(!first.pending_auto_transfer.contains_key("project-256"));

        store
            .append(event(
                PROMPT_LEARNING_OUTBOX_MAX_PENDING as u64 + 2,
                "dispatch",
                Some("project-000"),
                Some("intent-000"),
                None,
            ))
            .unwrap();
        let refilled =
            load_prompt_learning_outbox_projection(&mut store, project_test_event, |_| true)
                .unwrap();
        assert!(!refilled.overflowed);
        assert_eq!(refilled.pending_len(), PROMPT_LEARNING_OUTBOX_MAX_PENDING);
        assert!(!refilled.pending_auto_transfer.contains_key("project-000"));
        assert!(refilled.pending_auto_transfer.contains_key("project-256"));
        assert!(!refilled.used_fast_revision_path());
    }

    #[test]
    fn persisted_cursor_rejects_an_internal_canonical_sequence_gap() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .append(event(
                1,
                "intent",
                Some("project"),
                Some("intent"),
                Some("payload"),
            ))
            .unwrap();
        let first =
            load_prompt_learning_outbox_projection(&mut store, project_test_event, |_| true)
                .unwrap();
        assert_eq!(first.event_count, 1);
        store
            .append(event(3, "unrelated", None, None, None))
            .unwrap();
        let error =
            load_prompt_learning_outbox_projection(&mut store, project_test_event, |_| true)
                .unwrap_err();
        assert!(error.contains("canonical replay changed twice"));
    }

    #[test]
    fn corrupted_payload_digest_forces_replay_from_canonical_events() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .append(event(
                1,
                "intent",
                Some("project"),
                Some("intent"),
                Some("canonical"),
            ))
            .unwrap();
        let projection =
            load_prompt_learning_outbox_projection(&mut store, project_test_event, |_| true)
                .unwrap();
        let mut corrupt = projection.clone();
        corrupt
            .pending_auto_transfer
            .get_mut("project")
            .unwrap()
            .payload = "corrupt".to_string();
        store
            .save_read_model(
                PROMPT_LEARNING_OUTBOX_SCHEMA,
                PROMPT_LEARNING_OUTBOX_KEY,
                corrupt.revision,
                &serde_json::to_string(&corrupt).unwrap(),
            )
            .unwrap();
        let rebuilt =
            load_prompt_learning_outbox_projection(&mut store, project_test_event, |_| true)
                .unwrap();
        assert_eq!(
            rebuilt.pending_auto_transfer().next(),
            Some(("project", "intent", "canonical"))
        );
    }

    #[test]
    fn stale_cas_never_overwrites_a_competing_snapshot() {
        let mut store = SqliteStore::in_memory().unwrap();
        let initial = PromptLearningOutboxProjection::empty();
        store
            .save_read_model(
                PROMPT_LEARNING_OUTBOX_SCHEMA,
                PROMPT_LEARNING_OUTBOX_KEY,
                0,
                &serde_json::to_string(&initial).unwrap(),
            )
            .unwrap();
        let expected = store
            .load_read_model(PROMPT_LEARNING_OUTBOX_SCHEMA, PROMPT_LEARNING_OUTBOX_KEY)
            .unwrap()
            .unwrap();
        store
            .save_read_model(
                PROMPT_LEARNING_OUTBOX_SCHEMA,
                PROMPT_LEARNING_OUTBOX_KEY,
                0,
                "competing",
            )
            .unwrap();
        assert!(!compare_exchange_prompt_learning_outbox_projection(
            &mut store,
            Some(&expected),
            &initial,
        )
        .unwrap());
    }

    #[test]
    fn existing_request_without_marker_is_marked_once_and_does_not_restart_pending() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .append(event(
                1,
                "intent",
                Some("project"),
                Some("intent"),
                Some("payload"),
            ))
            .unwrap();
        let pending =
            load_prompt_learning_outbox_projection(&mut store, project_test_event, |_| true)
                .unwrap();
        assert_eq!(pending.pending_len(), 1);
        assert_eq!(
            prompt_learning_dispatch_recovery(true),
            PromptLearningDispatchRecovery::MarkExistingRequest
        );
        store
            .append(event(2, "dispatch", Some("project"), Some("intent"), None))
            .unwrap();
        let restarted =
            load_prompt_learning_outbox_projection(&mut store, project_test_event, |_| true)
                .unwrap();
        assert_eq!(restarted.pending_len(), 0);
        assert_eq!(
            store
                .list_by_task(&crate::runtime_values::phase16_task_id())
                .unwrap()
                .iter()
                .filter(|event| event.summary == "dispatch")
                .count(),
            1
        );
    }

    #[test]
    fn prompt_learning_outbox_delta_projection_scaling_gate() {
        const HISTORY_EVENTS: u64 = 4_096;
        let mut store = SqliteStore::in_memory().unwrap();
        for sequence in 1..HISTORY_EVENTS {
            store
                .append(event(sequence, "unrelated", None, None, None))
                .unwrap();
        }
        store
            .append(event(
                HISTORY_EVENTS,
                "intent",
                Some("project"),
                Some("stable-intent"),
                Some("stable-payload"),
            ))
            .unwrap();
        let initial_visits = Cell::new(0usize);
        let initial = load_prompt_learning_outbox_projection(
            &mut store,
            |projection, event| {
                initial_visits.set(initial_visits.get().saturating_add(1));
                project_test_event(projection, event)
            },
            |_| true,
        )
        .unwrap();
        let pending_before = initial
            .pending_auto_transfer()
            .map(|(project_id, intent_id, payload)| {
                (
                    project_id.to_string(),
                    intent_id.to_string(),
                    payload.to_string(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(initial_visits.get(), HISTORY_EVENTS as usize);

        store
            .append(event(HISTORY_EVENTS + 1, "unrelated", None, None, None))
            .unwrap();
        let delta_visits = Cell::new(0usize);
        let updated = load_prompt_learning_outbox_projection(
            &mut store,
            |projection, event| {
                delta_visits.set(delta_visits.get().saturating_add(1));
                project_test_event(projection, event)
            },
            |_| true,
        )
        .unwrap();
        let pending_after = updated
            .pending_auto_transfer()
            .map(|(project_id, intent_id, payload)| {
                (
                    project_id.to_string(),
                    intent_id.to_string(),
                    payload.to_string(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(delta_visits.get(), 1);
        assert_eq!(pending_after, pending_before);
        assert!(updated.used_fast_revision_path());
        println!(
            "{}",
            serde_json::json!({
                "schema": "cindx.prompt-learning-outbox-scaling.v1",
                "history_events": HISTORY_EVENTS,
                "delta_events": 1,
                "projector_visits": delta_visits.get(),
                "fast_revision_path": updated.used_fast_revision_path(),
                "pending_identity_preserved": pending_after == pending_before,
            })
        );
    }
}
