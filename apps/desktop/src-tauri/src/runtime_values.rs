use crate::{
    configuration_models::ProviderConfig,
    runtime_constants::{
        LEGACY_AGENT_SYSTEM_PROMPT, NEXT_ID, PHASE15_TASK_ID, PHASE16_TASK_ID, PHASE3_TASK_ID,
        PHASE4_TASK_ID, PHASE5_TASK_ID, PHASE6_TASK_ID, PHASE7_TASK_ID, PHASE8_TASK_ID,
    },
};
use agent_core::{
    Event, EventKind, MessageRole, Metadata, PermissionDecision, TaskId, ToolOutcomeStatus,
    ToolRisk,
};
use agent_runtime::compose_agent_system_prompt;
use agent_storage::StorageError;
use std::{
    sync::atomic::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};
use tools::prompt_requests_image_generation;

pub(crate) fn phase3_task_id() -> TaskId {
    TaskId(PHASE3_TASK_ID.to_string())
}

pub(crate) fn phase4_task_id() -> TaskId {
    TaskId(PHASE4_TASK_ID.to_string())
}

pub(crate) fn phase5_task_id() -> TaskId {
    TaskId(PHASE5_TASK_ID.to_string())
}

pub(crate) fn phase6_task_id() -> TaskId {
    TaskId(PHASE6_TASK_ID.to_string())
}

pub(crate) fn phase7_task_id() -> TaskId {
    TaskId(PHASE7_TASK_ID.to_string())
}

pub(crate) fn phase8_task_id() -> TaskId {
    TaskId(PHASE8_TASK_ID.to_string())
}

pub(crate) fn phase15_task_id() -> TaskId {
    TaskId(PHASE15_TASK_ID.to_string())
}

pub(crate) fn phase16_task_id() -> TaskId {
    TaskId(PHASE16_TASK_ID.to_string())
}

pub(crate) fn context_task_ids() -> Vec<TaskId> {
    vec![
        phase3_task_id(),
        phase4_task_id(),
        phase5_task_id(),
        phase6_task_id(),
        phase7_task_id(),
        phase8_task_id(),
        phase15_task_id(),
        phase16_task_id(),
    ]
}

pub(crate) fn unique_id(prefix: &str) -> String {
    let counter = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", current_time_millis())
}

pub(crate) fn new_session_id() -> String {
    format!("sess_{}", uuid::Uuid::now_v7())
}

pub(crate) fn new_project_id(_name: &str) -> String {
    format!("project-{}", uuid::Uuid::now_v7())
}

pub(crate) fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_millis() as u64
}

pub(crate) fn run_context_steer_epoch(run_context: &Metadata) -> u64 {
    run_context
        .get("steer_epoch")
        .and_then(|epoch| epoch.parse::<u64>().ok())
        .unwrap_or_default()
}

pub(crate) fn parse_permission_decision(value: &str) -> Result<PermissionDecision, StorageError> {
    match value {
        "allow_once" => Ok(PermissionDecision::AllowOnce),
        "allow_for_session" => Ok(PermissionDecision::AllowForSession),
        "deny" => Ok(PermissionDecision::Deny),
        other => Err(StorageError::new(format!(
            "unknown permission decision: {other}"
        ))),
    }
}

pub(crate) fn event_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::TaskStatusChanged => "Status",
        EventKind::ToolCallProposed => "Tool proposed",
        EventKind::PermissionRequested => "Permission requested",
        EventKind::PermissionResolved => "Permission resolved",
        EventKind::ToolCallStarted => "Tool started",
        EventKind::ToolCallFinished => "Tool finished",
        EventKind::ModelRequestStarted => "Model started",
        EventKind::ModelRequestFinished => "Model finished",
        EventKind::RetrievalPerformed => "Retrieval",
        EventKind::MessageAdded => "Message",
        EventKind::Error => "Error",
        _ => "Runtime event",
    }
}

pub(crate) fn timeline_event_label(event: &Event) -> String {
    if matches!(
        event.kind,
        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished
    ) && event.metadata.contains_key("collaboration_id")
    {
        if let Some(stage) = event.metadata.get("stage") {
            return collaboration_stage_display_label(stage);
        }
    }
    event_kind_label(&event.kind).to_string()
}

pub(crate) fn collaboration_stage_display_label(stage: &str) -> String {
    match stage {
        "coordinator" => "Conductor".to_string(),
        "conductor_plan" => "Conductor".to_string(),
        "conductor_repair" => "Conductor repair".to_string(),
        "planner" => "Planner".to_string(),
        "arbiter" => "Arbiter".to_string(),
        "executor" => "Executor".to_string(),
        "reviewer" => "Reviewer".to_string(),
        "synthesizer" => "Synthesis".to_string(),
        _ => {
            if let Some(index) = stage.strip_prefix("candidate_") {
                format!("Candidate {index}")
            } else if let Some(index) = stage.strip_prefix("worker_") {
                format!("Worker {index}")
            } else {
                "Model collaboration".to_string()
            }
        }
    }
}

pub(crate) fn event_kind_ui_kind(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::ToolCallProposed | EventKind::ToolCallStarted | EventKind::ToolCallFinished => {
            "tool"
        }
        EventKind::PermissionRequested | EventKind::PermissionResolved => "permission",
        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished => "model",
        EventKind::RetrievalPerformed => "tool",
        _ => "message",
    }
}

pub(crate) fn event_state(kind: &EventKind, permission_is_pending: bool) -> &'static str {
    match kind {
        EventKind::PermissionRequested if permission_is_pending => "pending",
        EventKind::ToolCallProposed if permission_is_pending => "pending",
        EventKind::Error => "pending",
        _ => "done",
    }
}

pub(crate) fn message_role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::Reviewer => "reviewer",
    }
}

pub(crate) fn message_role_from_label(value: &str) -> Option<MessageRole> {
    match value {
        "system" => Some(MessageRole::System),
        "user" => Some(MessageRole::User),
        "assistant" => Some(MessageRole::Assistant),
        "tool" => Some(MessageRole::Tool),
        "reviewer" => Some(MessageRole::Reviewer),
        _ => None,
    }
}

pub(crate) fn tool_risk_label(risk: &ToolRisk) -> &'static str {
    match risk {
        ToolRisk::ReadOnly => "read_only",
        ToolRisk::WritesWorkspace => "writes_workspace",
        ToolRisk::ExecutesProcess => "executes_process",
        ToolRisk::UsesNetwork => "uses_network",
        ToolRisk::SensitiveContext => "sensitive_context",
        ToolRisk::Destructive => "destructive",
    }
}

pub(crate) fn tool_outcome_label(status: &ToolOutcomeStatus) -> &'static str {
    match status {
        ToolOutcomeStatus::Succeeded => "succeeded",
        ToolOutcomeStatus::Failed => "failed",
        ToolOutcomeStatus::Cancelled => "cancelled",
        ToolOutcomeStatus::Denied => "denied",
    }
}

pub(crate) fn normalized_config_value(value: &str) -> String {
    sanitize_config_value(value.trim())
}

pub(crate) fn normalized_agent_instructions(value: &str) -> String {
    let prompt = value.trim();
    if prompt.is_empty() || prompt == LEGACY_AGENT_SYSTEM_PROMPT {
        String::new()
    } else {
        prompt.chars().take(32_000).collect()
    }
}

pub(crate) fn normalized_current_time_context(value: &str) -> String {
    let context = sanitize_config_value(value.trim())
        .chars()
        .take(256)
        .collect::<String>();
    if context.is_empty() {
        format!("Unix time {} ms (UTC)", current_time_millis())
    } else {
        context
    }
}

pub(crate) fn add_image_generation_run_context(
    run_context: &mut Metadata,
    config: &ProviderConfig,
    prompt: &str,
) {
    run_context.remove("image_generation_required");
    run_context.remove("configured_image_model");
    run_context.remove("configured_image_endpoint");
    if !prompt_requests_image_generation(prompt) || config.image_model.trim().is_empty() {
        return;
    }
    run_context.insert("image_generation_required".to_string(), "true".to_string());
    run_context.insert(
        "configured_image_model".to_string(),
        config.image_model.trim().to_string(),
    );
    run_context.insert(
        "configured_image_endpoint".to_string(),
        if config.image_endpoint.trim().is_empty() {
            config.base_url.trim().to_string()
        } else {
            config.image_endpoint.trim().to_string()
        },
    );
}

pub(crate) fn effective_agent_objective<'a>(
    run_context: &'a Metadata,
    latest_prompt: &'a str,
) -> &'a str {
    run_context
        .get("effective_prompt_objective")
        .map(String::as_str)
        .filter(|objective| !objective.trim().is_empty())
        .unwrap_or(latest_prompt)
}

pub(crate) fn agent_runtime_context_for_run(run_context: &Metadata) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(current_time) = run_context.get("current_time") {
        sections.push(format!(
            "Current date and time: {current_time}\nTreat this time as authoritative for this turn."
        ));
    }
    if run_context
        .get("image_generation_required")
        .map(String::as_str)
        == Some("true")
    {
        let model = run_context
            .get("configured_image_model")
            .map(String::as_str)
            .unwrap_or("the configured image model");
        let endpoint = run_context
            .get("configured_image_endpoint")
            .map(String::as_str)
            .unwrap_or("the configured image endpoint");
        sections.push(format!(
            "Image generation policy (authoritative): this request requires raster image generation. You MUST use `image.generate`, which is locked to the user's Settings model `{model}` at `{endpoint}`. Never substitute a model, provider, shell command, browser workflow, direct HTTP request, SVG, emoji, CSS drawing, or text-only approximation for the requested generated image. The visual prompt is your responsibility; model and provider selection are not."
        ));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

pub(crate) fn collaboration_system_prompt_for_run(
    user_instructions: &str,
    run_context: &Metadata,
) -> String {
    let runtime_context = agent_runtime_context_for_run(run_context);
    compose_agent_system_prompt(Some(user_instructions), runtime_context.as_deref())
}

pub(crate) fn config_hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn config_hex_decode(value: &str) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

pub(crate) fn sanitize_config_value(value: &str) -> String {
    value.replace(['\n', '\r'], "")
}

pub(crate) fn sanitize_record_field(value: &str) -> String {
    sanitize_config_value(value).replace('\t', " ")
}

pub(crate) fn unique_config_id(prefix: &str, label: &str, existing: &[String]) -> String {
    let slug = slug_label(label);
    let base = format!("{prefix}-{slug}");
    if !existing.iter().any(|id| id == &base) {
        return base;
    }
    for index in 2..1000 {
        let candidate = format!("{base}-{index}");
        if !existing.iter().any(|id| id == &candidate) {
            return candidate;
        }
    }

    format!("{base}-{}", current_time_millis())
}

pub(crate) fn slug_label(label: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "item".to_string()
    } else {
        slug
    }
}

pub(crate) fn truncate_for_timeline(value: &str) -> String {
    const LIMIT: usize = 160;
    if value.chars().count() <= LIMIT {
        return value.to_string();
    }

    let mut truncated = value.chars().take(LIMIT).collect::<String>();
    truncated.push_str("...");
    truncated
}
