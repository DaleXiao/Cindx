use agent_core::Event;
use agent_storage::{SqliteStore, StoredReadModel};
use orchestrator::{PromptLearningOutboxProjection, PROMPT_LEARNING_OUTBOX_SCHEMA};

#[cfg(test)]
use orchestrator::PROMPT_LEARNING_OUTBOX_MAX_PENDING;

const PROMPT_LEARNING_OUTBOX_KEY: &str = "global";

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
    projection.set_cursor(revision.latest_sequence, revision.event_count);
    Ok(Some(projection))
}

pub(crate) fn load_prompt_learning_outbox_projection(
    store: &mut SqliteStore,
    mut project_event: impl FnMut(&mut PromptLearningOutboxProjection, &Event) -> Result<(), String>,
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
                        && projection.revision() == stored.revision
                        && projection.revision() <= latest_sequence
                })
        });
        let mut changed = stored.is_none();
        let mut full_replay_required = stored.is_none();
        let mut projection = stored.unwrap_or_else(PromptLearningOutboxProjection::empty);
        if !full_replay_required {
            let delta = store
                .list_by_task_after(&task_id, projection.revision())
                .map_err(|error| error.to_string())?;
            if !delta_is_contiguous(&delta, projection.revision(), latest_sequence) {
                full_replay_required = true;
            } else {
                changed |= !delta.is_empty();
                for event in &delta {
                    project_event(&mut projection, event)?;
                }
                if projection.overflow_recovery_required() {
                    full_replay_required = true;
                } else {
                    projection.set_fast_tail_cursor(
                        latest_sequence,
                        projection.event_count().saturating_add(delta.len() as u64),
                    );
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
        if !projection.is_intrinsically_valid() {
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
    if projection.revision() != latest_sequence || !projection.is_intrinsically_valid() {
        return Ok(false);
    }
    let payload = serde_json::to_string(projection)
        .map_err(|error| format!("prompt learning outbox serialization failed: {error}"))?;
    store
        .compare_exchange_read_model(
            PROMPT_LEARNING_OUTBOX_SCHEMA,
            PROMPT_LEARNING_OUTBOX_KEY,
            expected,
            projection.revision(),
            &payload,
        )
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind, Metadata, TaskId};
    use agent_storage::EventStore;
    use orchestrator::PromptAutoTransferIntent;
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
            "intent" => projection.insert_auto_transfer_intent(
                event.sequence,
                &test_auto_intent(event.metadata.get("project_id").unwrap()),
            ),
            "replacement" => projection.insert_auto_transfer_intent(
                event.sequence,
                &test_auto_intent_with_run(
                    event.metadata.get("project_id").unwrap(),
                    "replacement-run",
                ),
            ),
            "dispatch" => projection.remove_auto_transfer(
                event.metadata.get("project_id").unwrap(),
                event.metadata.get("intent_id").unwrap(),
            ),
            _ => Ok(()),
        }
    }

    fn test_auto_intent(project_id: &str) -> PromptAutoTransferIntent {
        test_auto_intent_with_run(project_id, &format!("run-{project_id}"))
    }

    fn test_auto_intent_with_run(project_id: &str, run_id: &str) -> PromptAutoTransferIntent {
        let context = [
            ("project_id".to_string(), project_id.to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
        ]
        .into_iter()
        .collect();
        PromptAutoTransferIntent::from_context(&TaskId("test-task".to_string()), &context).unwrap()
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
            load_prompt_learning_outbox_projection(&mut store, project_test_event)
                .unwrap();
        let first_intent = test_auto_intent("project-000");
        let overflow_intent = test_auto_intent("project-256");
        assert!(first.overflowed());
        assert_eq!(first.pending_len(), PROMPT_LEARNING_OUTBOX_MAX_PENDING);
        assert!(first.contains_auto_transfer("project-000", first_intent.intent_id()));
        assert!(!first.contains_auto_transfer("project-256", overflow_intent.intent_id()));

        store
            .append(event(
                PROMPT_LEARNING_OUTBOX_MAX_PENDING as u64 + 2,
                "dispatch",
                Some("project-000"),
                Some(first_intent.intent_id()),
                None,
            ))
            .unwrap();
        let refilled =
            load_prompt_learning_outbox_projection(&mut store, project_test_event)
                .unwrap();
        assert!(!refilled.overflowed());
        assert_eq!(refilled.pending_len(), PROMPT_LEARNING_OUTBOX_MAX_PENDING);
        assert!(!refilled.contains_auto_transfer("project-000", first_intent.intent_id()));
        assert!(refilled.contains_auto_transfer("project-256", overflow_intent.intent_id()));
        assert!(!refilled.used_fast_revision_path());
    }

    #[test]
    fn overflowed_project_replacement_replays_before_dispatch_order_changes() {
        let mut store = SqliteStore::in_memory().unwrap();
        for index in 0..=PROMPT_LEARNING_OUTBOX_MAX_PENDING {
            store
                .append(event(
                    index as u64 + 1,
                    "intent",
                    Some(&format!("project-{index:03}")),
                    None,
                    None,
                ))
                .unwrap();
        }
        let initial =
            load_prompt_learning_outbox_projection(&mut store, project_test_event).unwrap();
        assert!(initial.overflowed());

        store
            .append(event(
                PROMPT_LEARNING_OUTBOX_MAX_PENDING as u64 + 2,
                "replacement",
                Some("project-000"),
                None,
                None,
            ))
            .unwrap();
        let replayed =
            load_prompt_learning_outbox_projection(&mut store, project_test_event).unwrap();
        let replacement = test_auto_intent_with_run("project-000", "replacement-run");
        let previously_hidden = test_auto_intent("project-256");

        assert!(!replayed.contains_auto_transfer("project-000", replacement.intent_id()));
        assert!(replayed.contains_auto_transfer("project-256", previously_hidden.intent_id()));
        assert!(!replayed.used_fast_revision_path());
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
            load_prompt_learning_outbox_projection(&mut store, project_test_event)
                .unwrap();
        assert_eq!(first.event_count(), 1);
        store
            .append(event(3, "unrelated", None, None, None))
            .unwrap();
        let error =
            load_prompt_learning_outbox_projection(&mut store, project_test_event)
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
            load_prompt_learning_outbox_projection(&mut store, project_test_event)
                .unwrap();
        let mut corrupt = serde_json::to_value(&projection).unwrap();
        corrupt["pending_auto_transfer"]["project"]["payload"] =
            serde_json::Value::String("corrupt".to_string());
        store
            .save_read_model(
                PROMPT_LEARNING_OUTBOX_SCHEMA,
                PROMPT_LEARNING_OUTBOX_KEY,
                projection.revision(),
                &serde_json::to_string(&corrupt).unwrap(),
            )
            .unwrap();
        let rebuilt =
            load_prompt_learning_outbox_projection(&mut store, project_test_event)
                .unwrap();
        let expected = test_auto_intent("project");
        let expected_payload = expected.to_json().unwrap();
        assert_eq!(
            rebuilt.pending_auto_transfer().next(),
            Some((
                "project",
                expected.intent_id(),
                expected_payload.as_str()
            ))
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
            load_prompt_learning_outbox_projection(&mut store, project_test_event)
                .unwrap();
        assert_eq!(pending.pending_len(), 1);
        assert_eq!(
            orchestrator::prompt_learning_dispatch_recovery(true),
            orchestrator::PromptLearningDispatchRecovery::MarkExistingRequest
        );
        let intent = test_auto_intent("project");
        store
            .append(event(
                2,
                "dispatch",
                Some("project"),
                Some(intent.intent_id()),
                None,
            ))
            .unwrap();
        let restarted =
            load_prompt_learning_outbox_projection(&mut store, project_test_event)
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
