use crate::event_security::{redact_metadata, redact_sensitive_text};
use crate::persisted_event_contract::tag_persisted_event_v1;
use crate::project_session_persistence::metadata_with_context;
use crate::runtime_values::{current_time_millis, message_role_label, unique_id};
use agent_core::{EventId, EventKind, Message, MessageRole, Metadata, TaskId};
use agent_runtime::sanitize_assistant_content;
use agent_storage::{SqliteStore, StorageError};

pub(crate) fn append_message_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    role: MessageRole,
    content: &str,
) -> Result<(), StorageError> {
    append_message_event_with_metadata(store, task_id, role, content, Metadata::new())
}

pub(crate) fn append_tool_message_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    tool_call_id: &str,
    tool_name: &str,
    status: &str,
    content: &str,
    run_context: Option<&Metadata>,
) -> Result<(), StorageError> {
    let metadata = [
        ("kind".to_string(), "tool_observation".to_string()),
        ("tool_call_id".to_string(), tool_call_id.to_string()),
        ("tool".to_string(), tool_name.to_string()),
        ("status".to_string(), status.to_string()),
    ]
    .into_iter()
    .collect();
    let metadata = match run_context {
        Some(context) => metadata_with_context(metadata, context),
        None => metadata,
    };
    append_message_event_with_metadata(store, task_id, MessageRole::Tool, content, metadata)
}

pub(crate) fn persist_new_runtime_messages(
    store: &mut SqliteStore,
    task_id: &TaskId,
    messages: &[Message],
    previous_message_count: usize,
    run_context: &Metadata,
) -> Result<(), StorageError> {
    for message in messages.iter().skip(previous_message_count) {
        append_message_event_with_metadata(
            store,
            task_id,
            message.role.clone(),
            &message.content,
            metadata_with_context(message.metadata.clone(), run_context),
        )?;
    }

    Ok(())
}

pub(crate) fn append_message_event_with_metadata(
    store: &mut SqliteStore,
    task_id: &TaskId,
    role: MessageRole,
    content: &str,
    mut metadata: Metadata,
) -> Result<(), StorageError> {
    let content = if role == MessageRole::Assistant {
        sanitize_assistant_content(content)
    } else {
        content.to_string()
    };
    if role == MessageRole::Assistant {
        if let Some(display_content) = metadata.get_mut("display_content") {
            *display_content = sanitize_assistant_content(display_content);
        }
    }
    metadata.insert("role".to_string(), message_role_label(&role).to_string());
    metadata.insert("content".to_string(), content.clone());
    metadata.insert("content_length".to_string(), content.len().to_string());

    append_event(
        store,
        task_id,
        EventKind::MessageAdded,
        format!("{} message", message_role_label(&role)),
        metadata,
    )
}

pub(crate) fn append_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    kind: EventKind,
    summary: impl Into<String>,
    metadata: Metadata,
) -> Result<(), StorageError> {
    let summary = summary.into();
    let mut metadata = redact_metadata(&metadata);
    tag_persisted_event_v1(&kind, &summary, &mut metadata);

    store.append_next_event(
        EventId(unique_id("event")),
        task_id.clone(),
        current_time_millis(),
        kind,
        redact_sensitive_text(&summary),
        metadata,
    )
}
