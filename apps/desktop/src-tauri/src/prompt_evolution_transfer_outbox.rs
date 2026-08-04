use crate::app_state::AppState;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_models::notify_prompt_evaluation_worker;
use agent_core::{Event, EventKind, Metadata, TaskId};
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::prompt_learning_outbox_projection::{
    load_prompt_learning_outbox_projection, prompt_learning_dispatch_recovery,
    PromptLearningDispatchRecovery, PromptLearningOutboxProjection,
};

const INTENT_SCHEMA: &str = "cindx.prompt-auto-transfer-intent.v1";
const INTENT_EVENT: &str = "Conductor prompt Auto transfer requested";
const DISPATCHED_EVENT: &str = "Conductor prompt Auto transfer dispatched";
const INTENT_KEY: &str = "prompt_auto_transfer_intent";
const INTENT_ID_KEY: &str = "prompt_auto_transfer_intent_id";
pub(crate) const AUTO_TRANSFER_REQUIRED_KEY: &str = "prompt_auto_transfer_required";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PromptAutoTransferIntent {
    schema: String,
    intent_id: String,
    task_id: String,
    run_context: Metadata,
}

impl PromptAutoTransferIntent {
    fn from_context(task_id: &TaskId, run_context: &Metadata) -> Option<Self> {
        let project_id = run_context.get("project_id")?.trim();
        let source_id = run_context
            .get("agent_run_id")
            .or_else(|| run_context.get("collaboration_id"))?
            .trim();
        if project_id.is_empty() || source_id.is_empty() {
            return None;
        }
        let steer_epoch = run_context
            .get("steer_epoch")
            .map(String::as_str)
            .unwrap_or("0");
        let digest = sha256_hex(
            format!("{}\n{project_id}\n{source_id}\n{steer_epoch}", task_id.0).as_bytes(),
        );
        Some(Self {
            schema: INTENT_SCHEMA.to_string(),
            intent_id: format!("prompt-auto-transfer-{digest}"),
            task_id: task_id.0.clone(),
            run_context: persistent_context(run_context),
        })
    }

    fn validate(&self) -> bool {
        self.schema == INTENT_SCHEMA
            && Self::from_context(&TaskId(self.task_id.clone()), &self.run_context).as_ref()
                == Some(self)
    }
}

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
        .list_by_task_and_metadata(task_id, INTENT_ID_KEY, &intent.intent_id)
        .map_err(|error| error.to_string())?;
    if existing.iter().any(|event| event.summary == INTENT_EVENT) {
        return Ok(());
    }
    let payload = serde_json::to_string(&intent)
        .map_err(|error| format!("prompt Auto transfer intent serialization failed: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        INTENT_EVENT,
        metadata_with_context(
            [
                (INTENT_ID_KEY.to_string(), intent.intent_id),
                (INTENT_KEY.to_string(), payload),
                ("background_evaluation".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &intent.run_context,
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
            decode_auto_transfer_intent(project_id, intent_id, payload)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut dispatched_any = false;
    for (intent_id, intent) in intents {
        let request_id = format!("prompt-evaluation-{intent_id}");
        let task_id = TaskId(intent.task_id.clone());
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
        {
            if !crate::prompt_pairwise_runtime::enqueue_prompt_auto_transfer_evaluation(
                app,
                &task_id,
                &intent.run_context,
                Some(request_id),
            )? {
                continue;
            }
        }
        append_dispatched(&state, &intent, &intent_id)?;
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
    load_prompt_learning_outbox_projection(
        store,
        project_prompt_learning_outbox_event,
        prompt_learning_outbox_payloads_are_valid,
    )
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
        let payload = serde_json::to_string(&intent).map_err(|error| {
            format!("prompt Auto transfer intent serialization failed: {error}")
        })?;
        let project_id = intent
            .run_context
            .get("project_id")
            .ok_or_else(|| "prompt Auto transfer project is missing".to_string())?;
        projection.insert_auto_transfer(project_id, &intent.intent_id, event.sequence, payload)?;
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
            let payload = serde_json::to_string(&intent).map_err(|error| {
                format!("prompt Auto transfer intent serialization failed: {error}")
            })?;
            let project_id = intent
                .run_context
                .get("project_id")
                .ok_or_else(|| "prompt Auto transfer project is missing".to_string())?;
            projection.insert_auto_transfer(
                project_id,
                &intent.intent_id,
                event.sequence,
                payload,
            )?;
        }
    }
    Ok(())
}

fn prompt_learning_outbox_payloads_are_valid(projection: &PromptLearningOutboxProjection) -> bool {
    projection
        .pending_auto_transfer()
        .all(|(project_id, intent_id, payload)| {
            decode_auto_transfer_intent(project_id, intent_id, payload).is_ok()
        })
        && projection
            .pending_pro_distillation()
            .all(|(project_id, intent_id, payload)| {
                crate::prompt_distillation_outbox::pro_distillation_intent_payload_is_valid(
                    project_id, intent_id, payload,
                )
            })
}

fn decode_auto_transfer_intent(
    project_id: &str,
    intent_id: &str,
    payload: &str,
) -> Result<(String, PromptAutoTransferIntent), String> {
    let intent = serde_json::from_str::<PromptAutoTransferIntent>(payload)
        .map_err(|error| format!("prompt Auto transfer intent is invalid: {error}"))?;
    if !intent.validate()
        || intent.intent_id != intent_id
        || intent.run_context.get("project_id").map(String::as_str) != Some(project_id)
    {
        return Err("prompt Auto transfer intent identity is invalid".to_string());
    }
    Ok((intent_id.to_string(), intent))
}

fn append_dispatched(
    state: &tauri::State<'_, AppState>,
    intent: &PromptAutoTransferIntent,
    intent_id: &str,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &TaskId(intent.task_id.clone()),
        EventKind::TaskStatusChanged,
        DISPATCHED_EVENT,
        metadata_with_context(
            [
                (INTENT_ID_KEY.to_string(), intent_id.to_string()),
                ("background_evaluation".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &intent.run_context,
        ),
    )
    .map_err(|error| error.to_string())
}

fn persistent_context(context: &Metadata) -> Metadata {
    [
        "agent_run_id",
        "collaboration_id",
        "project_id",
        "project_root",
        "session_id",
        "steer_epoch",
    ]
    .into_iter()
    .filter_map(|key| {
        context
            .get(key)
            .map(|value| (key.to_string(), value.clone()))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind};
    use agent_storage::{EventStore, SqliteStore};

    #[test]
    fn intent_identity_is_stable_for_the_same_auto_completion() {
        let context = [
            ("project_id".to_string(), "project".to_string()),
            ("agent_run_id".to_string(), "run".to_string()),
            ("steer_epoch".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect();
        let task_id = TaskId("task".to_string());

        let first = PromptAutoTransferIntent::from_context(&task_id, &context).unwrap();
        let second = PromptAutoTransferIntent::from_context(&task_id, &context).unwrap();

        assert_eq!(first.intent_id, second.intent_id);
    }

    #[test]
    fn intent_validation_rejects_malformed_or_cross_project_identity() {
        let context = [
            ("project_id".to_string(), "project-a".to_string()),
            ("agent_run_id".to_string(), "run".to_string()),
            ("steer_epoch".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect();
        let task_id = TaskId("task".to_string());
        let intent = PromptAutoTransferIntent::from_context(&task_id, &context).unwrap();
        assert!(intent.validate());

        let mut malformed_id = intent.clone();
        malformed_id.intent_id.push_str("-tampered");
        assert!(!malformed_id.validate());

        let mut cross_project = intent.clone();
        cross_project
            .run_context
            .insert("project_id".to_string(), "project-b".to_string());
        assert!(!cross_project.validate());

        let mut different_task = intent.clone();
        different_task.task_id = "other-task".to_string();
        assert!(!different_task.validate());

        let mut unexpected_context = intent;
        unexpected_context
            .run_context
            .insert("unexpected".to_string(), "value".to_string());
        assert!(!unexpected_context.validate());
    }

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
        intent_metadata.insert(INTENT_ID_KEY.to_string(), intent.intent_id.clone());
        intent_metadata.insert(
            INTENT_KEY.to_string(),
            serde_json::to_string(&intent).unwrap(),
        );
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
                    (INTENT_ID_KEY.to_string(), intent.intent_id),
                    ("project_id".to_string(), "project".to_string()),
                ]
                .into_iter()
                .collect(),
            })
            .unwrap();
        let restarted = load_prompt_learning_outbox(&mut store).unwrap();
        assert_eq!(restarted.pending_auto_transfer().count(), 0);
        assert_eq!(restarted.revision, 3);
        assert_eq!(restarted.event_count, 3);
    }
}
