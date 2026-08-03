use crate::app_state::AppState;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_models::notify_prompt_evaluation_worker;
use crate::runtime_values::phase16_task_id;
use agent_core::{EventKind, Metadata, TaskId};
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tauri::Manager;

const INTENT_SCHEMA: &str = "cindx.prompt-auto-transfer-intent.v1";
const INTENT_EVENT: &str = "Conductor prompt Auto transfer requested";
const DISPATCHED_EVENT: &str = "Conductor prompt Auto transfer dispatched";
const INTENT_KEY: &str = "prompt_auto_transfer_intent";
const INTENT_ID_KEY: &str = "prompt_auto_transfer_intent_id";
pub(crate) const AUTO_TRANSFER_REQUIRED_KEY: &str = "prompt_auto_transfer_required";

#[derive(Clone, Debug, Serialize, Deserialize)]
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
            && !self.intent_id.trim().is_empty()
            && !self.task_id.trim().is_empty()
            && self.run_context.contains_key("project_id")
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
    let (intent_events, completion_events) = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        (
            store
                .list_by_task_and_metadata(&phase16_task_id(), "background_evaluation", "true")
                .map_err(|error| error.to_string())?,
            store
                .list_by_task_and_metadata(&phase16_task_id(), AUTO_TRANSFER_REQUIRED_KEY, "true")
                .map_err(|error| error.to_string())?,
        )
    };
    let mut intents = BTreeMap::<String, PromptAutoTransferIntent>::new();
    let mut dispatched = BTreeSet::new();
    for event in &intent_events {
        if event.summary == DISPATCHED_EVENT {
            if let Some(intent_id) = event.metadata.get(INTENT_ID_KEY) {
                dispatched.insert(intent_id.clone());
            }
        } else if event.summary == INTENT_EVENT {
            let intent = event
                .metadata
                .get(INTENT_KEY)
                .and_then(|payload| serde_json::from_str::<PromptAutoTransferIntent>(payload).ok())
                .filter(PromptAutoTransferIntent::validate)
                .ok_or_else(|| "prompt Auto transfer intent is invalid".to_string())?;
            intents.entry(intent.intent_id.clone()).or_insert(intent);
        }
    }
    for event in completion_events {
        if let Some(intent) =
            PromptAutoTransferIntent::from_context(&event.task_id, &event.metadata)
        {
            intents.entry(intent.intent_id.clone()).or_insert(intent);
        }
    }
    for (intent_id, intent) in intents {
        if dispatched.contains(&intent_id) {
            continue;
        }
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
        if !request_exists
            && !crate::prompt_pairwise_runtime::enqueue_prompt_auto_transfer_evaluation(
                app,
                &task_id,
                &intent.run_context,
                Some(request_id),
            )?
        {
            continue;
        }
        append_dispatched(&state, &intent, &intent_id)?;
    }
    Ok(())
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
}
