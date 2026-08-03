use crate::app_state::AppState;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_campaign_budget::PromptEvaluationCampaignUsage;
use agent_core::{EventKind, Metadata, TaskId};

pub(crate) const ACTION_STARTED_EVENT: &str = "Conductor prompt evaluation action started";
pub(crate) const ACTION_COMPLETED_EVENT: &str = "Conductor prompt evaluation action completed";
pub(crate) const ACTION_ID_KEY: &str = "prompt_evaluation_action_id";
pub(crate) const ACTION_INDEX_KEY: &str = "prompt_evaluation_action_index";

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_prompt_evaluation_action(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    request_id: &str,
    effort: &str,
    action_id: &str,
    action_index: usize,
    completed_actions: usize,
    campaign_usage: Option<&PromptEvaluationCampaignUsage>,
) -> Result<(), String> {
    let summary = if campaign_usage.is_some() {
        ACTION_COMPLETED_EVENT
    } else {
        ACTION_STARTED_EVENT
    };
    let mut metadata = [
        (
            "prompt_evaluation_request_id".to_string(),
            request_id.to_string(),
        ),
        (ACTION_ID_KEY.to_string(), action_id.to_string()),
        (ACTION_INDEX_KEY.to_string(), action_index.to_string()),
        (
            "completed_actions".to_string(),
            completed_actions.to_string(),
        ),
        ("background_evaluation".to_string(), "true".to_string()),
        ("prompt_effort".to_string(), effort.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(usage) = campaign_usage {
        metadata.insert(
            "campaign_usage".to_string(),
            serde_json::to_string(usage)
                .map_err(|error| format!("prompt campaign usage serialization failed: {error}"))?,
        );
    }
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        summary,
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())
}
