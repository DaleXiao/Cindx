use crate::app_state::AppState;
use crate::background_work_runtime::foreground_agent_should_preempt;
use crate::configuration_models::ProviderConfig;
use crate::event_persistence::append_event;
use crate::memory_runtime::{
    memory_events_for_terminal_steer_epoch, refresh_project_memory_after_run,
    schedule_project_memory_vector_refresh,
};
use crate::project_session_persistence::metadata_with_context;
use crate::runtime_values::phase16_task_id;
use agent_application::{AgentRunEvent, AgentRunStatus};
use agent_core::{Event, EventKind, Message, MessageRole, Metadata, ModelRole};
use agent_memory::{
    parse_semantic_memory_batch, semantic_memory_extraction_prompt, validate_semantic_memory_batch,
};
use model_provider::{
    ModelCallMode, ModelRequest, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    MODEL_REQUEST_CANCELLED,
};
use std::path::PathBuf;

pub(crate) fn contains_completed_agent_run(events: &[Event]) -> bool {
    events.iter().any(|event| {
        AgentRunEvent::from_event(event).map(AgentRunEvent::status)
            == Some(AgentRunStatus::Completed)
    })
}

pub(crate) fn generate_semantic_memory(
    state: &tauri::State<'_, AppState>,
    workspace_root: &PathBuf,
    config: &ProviderConfig,
    run_context: &Metadata,
) -> Result<(), String> {
    let run_id = run_context
        .get("agent_run_id")
        .ok_or_else(|| "semantic memory requires an agent run id".to_string())?;
    let project_id = run_context
        .get("project_id")
        .ok_or_else(|| "semantic memory requires a project id".to_string())?;
    let session_id = run_context
        .get("session_id")
        .ok_or_else(|| "semantic memory requires a session id".to_string())?;
    let task_id = phase16_task_id();
    let events = memory_events_for_terminal_steer_epoch({
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .list_by_task_and_metadata(&task_id, "agent_run_id", run_id)
            .map_err(|error| error.to_string())?
    });
    if !contains_completed_agent_run(&events) {
        return Err("semantic memory skipped because the run is not complete".to_string());
    }

    let model = config.model_for_role(&ModelRole::Summarizer);
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: 45,
    });
    let request = ModelRequest {
        role: ModelRole::Summarizer,
        messages: vec![Message {
            role: MessageRole::User,
            content: semantic_memory_extraction_prompt(&events),
            metadata: Metadata::new(),
        }],
        tools: Vec::new(),
        mode: ModelCallMode::Streaming,
        metadata: [("max_output_tokens".to_string(), "1800".to_string())]
            .into_iter()
            .collect(),
    };
    let response = provider
        .complete_streaming_cancellable(request, |_| {}, || foreground_agent_should_preempt(state))
        .map_err(|error| {
            if error.is_cancelled() {
                MODEL_REQUEST_CANCELLED.to_string()
            } else {
                format!("semantic memory model failed: {error}")
            }
        })?;
    let batch = parse_semantic_memory_batch(&response.message.content)?;
    let validation = validate_semantic_memory_batch(batch.clone(), &events, project_id, session_id);
    let payload = serde_json::to_string(&batch)
        .map_err(|error| format!("semantic memory serialization failed: {error}"))?;

    let ledger = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Semantic memory candidates accepted",
            metadata_with_context(
                [
                    ("memory_candidates_json".to_string(), payload),
                    (
                        "memory_candidates_proposed".to_string(),
                        batch.candidates.len().to_string(),
                    ),
                    (
                        "memory_candidates_accepted".to_string(),
                        validation.accepted.len().to_string(),
                    ),
                    (
                        "memory_candidates_rejected".to_string(),
                        validation.rejected.to_string(),
                    ),
                    ("memory_extractor_model".to_string(), model),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        refresh_project_memory_after_run(&mut store, run_context)
            .map_err(|error| error.to_string())?
    };
    if let Some(ledger) = ledger {
        schedule_project_memory_vector_refresh(workspace_root.clone(), config.clone(), ledger);
    }
    Ok(())
}

pub(crate) fn recover_semantic_memory_without_model(
    state: &tauri::State<'_, AppState>,
    workspace_root: PathBuf,
    config: ProviderConfig,
    run_context: &Metadata,
    error: &str,
) {
    eprintln!("semantic project memory unavailable: {error}");
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::Error,
            "Semantic memory extraction unavailable",
            metadata_with_context(
                [("error".to_string(), bounded_chars(error, 800))]
                    .into_iter()
                    .collect(),
                run_context,
            ),
        );
        if let Ok(Some(ledger)) = refresh_project_memory_after_run(&mut store, run_context) {
            drop(store);
            schedule_project_memory_vector_refresh(workspace_root, config, ledger);
        }
    }
}

fn bounded_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let bounded = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{bounded} [truncated]")
    } else {
        bounded
    }
}
