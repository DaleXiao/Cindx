use crate::app_state::AppState;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_models::notify_prompt_evaluation_worker;
use agent_core::{Event, EventKind, Metadata, TaskId};
use orchestrator::{
    prompt_learning_dispatch_recovery, PromptAutoTransferIntent, PromptLearningDispatchRecovery,
    PromptLearningOutboxProjection,
};
use tauri::Manager;

use crate::prompt_learning_outbox_projection::load_prompt_learning_outbox_projection;

const INTENT_EVENT: &str = "Conductor prompt Auto transfer requested";
const DISPATCHED_EVENT: &str = "Conductor prompt Auto transfer dispatched";
const INTENT_KEY: &str = "prompt_auto_transfer_intent";
const INTENT_ID_KEY: &str = "prompt_auto_transfer_intent_id";
pub(crate) const AUTO_TRANSFER_REQUIRED_KEY: &str = "prompt_auto_transfer_required";

pub(crate) fn persist_prompt_auto_transfer_intent(
    app: &tauri::AppHandle,
    task_id: &TaskId,
    run_context: &Metadata,
) -> Result<(), String> {
    let Some(intent) = PromptAutoTransferIntent::from_context(task_id, run_context) else {
        return Ok(());
    };
    let state = app.state::<AppState>();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let existing = store
        .list_by_task_and_metadata(task_id, INTENT_ID_KEY, intent.intent_id())
        .map_err(|error| error.to_string())?;
    if existing.iter().any(|event| event.summary == INTENT_EVENT) {
        return Ok(());
    }
    let payload = intent.to_json()?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        INTENT_EVENT,
        metadata_with_context(
            [
                (INTENT_ID_KEY.to_string(), intent.intent_id().to_string()),
                (INTENT_KEY.to_string(), payload),
                ("background_evaluation".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            intent.run_context(),
        ),
    )
    .map_err(|error| error.to_string())?;
    drop(store);
    notify_prompt_evaluation_worker();
    Ok(())
}

pub(crate) fn dispatch_prompt_auto_transfer_intents(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let projection = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        load_prompt_learning_outbox(&mut store)?
    };
    let intents = projection
        .pending_auto_transfer()
        .map(|(project_id, intent_id, payload)| {
            PromptAutoTransferIntent::decode_for_project(project_id, intent_id, payload)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut dispatched_any = false;
    for intent in intents {
        let request_id = format!("prompt-evaluation-{}", intent.intent_id());
        let task_id = TaskId(intent.task_id().to_string());
        let request_exists = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?
            .list_by_task_and_metadata(&task_id, "prompt_evaluation_request_id", &request_id)
            .map_err(|error| error.to_string())?
            .iter()
            .any(|event| event.summary == "Conductor prompt evaluation requested");
        if prompt_learning_dispatch_recovery(request_exists)
            == PromptLearningDispatchRecovery::EnqueueThenMark
            && !crate::prompt_pairwise_runtime::enqueue_prompt_auto_transfer_evaluation(
                app,
                &task_id,
                intent.run_context(),
                Some(request_id),
            )?
        {
            continue;
        }
        append_dispatched(&state, &intent)?;
        dispatched_any = true;
    }
    if dispatched_any {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        load_prompt_learning_outbox(&mut store)?;
    }
    Ok(())
}

pub(crate) fn load_prompt_learning_outbox(
    store: &mut agent_storage::SqliteStore,
) -> Result<PromptLearningOutboxProjection, String> {
    load_prompt_learning_outbox_projection(store, project_prompt_learning_outbox_event)
}

fn project_prompt_learning_outbox_event(
    projection: &mut PromptLearningOutboxProjection,
    event: &Event,
) -> Result<(), String> {
    project_auto_transfer_outbox_event(projection, event)?;
    crate::prompt_distillation_outbox::project_pro_distillation_outbox_event(projection, event)
}

fn project_auto_transfer_outbox_event(
    projection: &mut PromptLearningOutboxProjection,
    event: &Event,
) -> Result<(), String> {
    if event.summary == DISPATCHED_EVENT {
        let intent_id = event
            .metadata
            .get(INTENT_ID_KEY)
            .ok_or_else(|| "prompt Auto transfer dispatch intent id is missing".to_string())?;
        let project_id = event
            .metadata
            .get("project_id")
            .ok_or_else(|| "prompt Auto transfer dispatch project is missing".to_string())?;
        return projection.remove_auto_transfer(project_id, intent_id);
    }
    if event.summary == INTENT_EVENT {
        let intent = event
            .metadata
            .get(INTENT_KEY)
            .and_then(|payload| serde_json::from_str::<PromptAutoTransferIntent>(payload).ok())
            .filter(PromptAutoTransferIntent::validate)
            .ok_or_else(|| "prompt Auto transfer intent is invalid".to_string())?;
        projection.insert_auto_transfer_intent(event.sequence, &intent)?;
    }
    if event
        .metadata
        .get(AUTO_TRANSFER_REQUIRED_KEY)
        .map(String::as_str)
        == Some("true")
    {
        if let Some(intent) =
            PromptAutoTransferIntent::from_context(&event.task_id, &event.metadata)
        {
            projection.insert_auto_transfer_intent(event.sequence, &intent)?;
        }
    }
    Ok(())
}

fn append_dispatched(
    state: &tauri::State<'_, AppState>,
    intent: &PromptAutoTransferIntent,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &TaskId(intent.task_id().to_string()),
        EventKind::TaskStatusChanged,
        DISPATCHED_EVENT,
        metadata_with_context(
            [
                (INTENT_ID_KEY.to_string(), intent.intent_id().to_string()),
                ("background_evaluation".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            intent.run_context(),
        ),
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind};
    use agent_storage::{EventStore, SqliteStore};

    #[test]
    fn canonical_completion_and_intent_replay_to_one_pending_entry() {
        let task_id = crate::runtime_values::phase16_task_id();
        let context = [
            ("project_id".to_string(), "project".to_string()),
            ("agent_run_id".to_string(), "run".to_string()),
            ("steer_epoch".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let intent = PromptAutoTransferIntent::from_context(&task_id, &context).unwrap();
        let mut completion_metadata = context.clone();
        completion_metadata.insert(AUTO_TRANSFER_REQUIRED_KEY.to_string(), "true".to_string());
        let mut intent_metadata = context;
        intent_metadata.insert(INTENT_ID_KEY.to_string(), intent.intent_id().to_string());
        intent_metadata.insert(INTENT_KEY.to_string(), intent.to_json().unwrap());
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .append(Event {
                id: EventId("completion".to_string()),
                task_id: task_id.clone(),
                sequence: 1,
                timestamp_ms: 1,
                kind: EventKind::TaskStatusChanged,
                summary: "Collaboration workflow completed".to_string(),
                metadata: completion_metadata,
            })
            .unwrap();
        store
            .append(Event {
                id: EventId("intent".to_string()),
                task_id: task_id.clone(),
                sequence: 2,
                timestamp_ms: 2,
                kind: EventKind::TaskStatusChanged,
                summary: INTENT_EVENT.to_string(),
                metadata: intent_metadata,
            })
            .unwrap();

        let projection = load_prompt_learning_outbox(&mut store).unwrap();
        assert_eq!(projection.pending_auto_transfer().count(), 1);

        store
            .append(Event {
                id: EventId("dispatch".to_string()),
                task_id,
                sequence: 3,
                timestamp_ms: 3,
                kind: EventKind::TaskStatusChanged,
                summary: DISPATCHED_EVENT.to_string(),
                metadata: [
                    (INTENT_ID_KEY.to_string(), intent.intent_id().to_string()),
                    ("project_id".to_string(), "project".to_string()),
                ]
                .into_iter()
                .collect(),
            })
            .unwrap();
        let restarted = load_prompt_learning_outbox(&mut store).unwrap();
        assert_eq!(restarted.pending_auto_transfer().count(), 0);
        assert_eq!(restarted.revision(), 3);
        assert_eq!(restarted.event_count(), 3);
    }
}
