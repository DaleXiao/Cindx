use super::*;

pub(crate) fn message_view_from_event(event: &Event) -> Option<ChatMessageView> {
    if event.kind != EventKind::MessageAdded {
        return None;
    }
    if event.metadata.get("internal").map(String::as_str) == Some("true") {
        return None;
    }

    let role = event.metadata.get("role")?.to_string();
    let mut content = redact_sensitive_text(
        event
            .metadata
            .get("display_content")
            .or_else(|| event.metadata.get("content"))?,
    );
    if role == "assistant" {
        content = sanitize_assistant_content(&content);
    }

    Some(ChatMessageView {
        sequence: event.sequence,
        role,
        content,
        timestamp_ms: event.timestamp_ms,
        run_id: event.metadata.get("agent_run_id").cloned(),
        queue_id: event.metadata.get("queue_id").cloned(),
        attachments: attachment_views_from_event(event),
    })
}

pub(crate) fn attachment_views_from_event(event: &Event) -> Vec<AgentAttachmentView> {
    let paths = event
        .metadata
        .get("attachment_paths")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    if paths.is_empty() {
        return Vec::new();
    }

    let names = event
        .metadata
        .get("attachment_names")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let ids = event
        .metadata
        .get("attachment_ids")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let mime_types = event
        .metadata
        .get("attachment_mime_types")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let sizes = event
        .metadata
        .get("attachment_sizes")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let image_paths = event
        .metadata
        .get("image_paths")
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();

    paths
        .into_iter()
        .enumerate()
        .filter(|(_, path)| !path.trim().is_empty())
        .map(|(index, path)| {
            let inferred_mime = normalized_attachment_mime("", Path::new(path));
            let mime_type = mime_types
                .get(index)
                .filter(|value| !value.trim().is_empty())
                .map(|value| (*value).to_string())
                .unwrap_or_else(|| {
                    if image_paths.contains(&path) && !inferred_mime.starts_with("image/") {
                        "image/*".to_string()
                    } else {
                        inferred_mime
                    }
                });
            AgentAttachmentView {
                id: ids
                    .get(index)
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| (*value).to_string())
                    .unwrap_or_else(|| format!("message-attachment-{}-{index}", event.sequence)),
                name: names
                    .get(index)
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| (*value).to_string())
                    .unwrap_or_else(|| safe_attachment_name(path)),
                path: path.to_string(),
                mime_type,
                size_bytes: sizes
                    .get(index)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or_default(),
            }
        })
        .collect()
}

pub(crate) fn message_from_event(event: &Event) -> Option<Message> {
    message_from_event_with_policy(event, false, false)
}

pub(crate) fn model_message_from_event(event: &Event) -> Option<Message> {
    message_from_event_with_policy(event, false, true)
}

pub(crate) fn runtime_message_from_event(event: &Event) -> Option<Message> {
    message_from_event_with_policy(event, true, true)
}

fn message_from_event_with_policy(
    event: &Event,
    include_internal: bool,
    prefer_model_content: bool,
) -> Option<Message> {
    if event.kind != EventKind::MessageAdded {
        return None;
    }
    if !include_internal
        && event.metadata.get("internal").map(String::as_str) == Some("true")
        && event.metadata.get("kind").map(String::as_str) != Some("visual_reference")
    {
        return None;
    }

    let role = message_role_from_label(event.metadata.get("role")?)?;
    let stored_content = if prefer_model_content && role == MessageRole::User {
        event
            .metadata
            .get("model_content")
            .or_else(|| event.metadata.get("content"))?
    } else {
        event.metadata.get("content")?
    };
    let mut content = redact_sensitive_text(stored_content);
    if role == MessageRole::Assistant {
        content = sanitize_assistant_content(&content);
    }

    let mut metadata = redact_metadata(&event.metadata);
    if role == MessageRole::Assistant {
        metadata.insert("content".to_string(), content.clone());
        if metadata.contains_key("display_content") {
            metadata.insert("display_content".to_string(), content.clone());
        }
    }

    Some(Message {
        role,
        content,
        metadata,
    })
}

pub(crate) fn tool_run_from_event(event: &Event) -> Option<ToolRunView> {
    if event.kind != EventKind::ToolCallFinished {
        return None;
    }

    Some(ToolRunView {
        invocation_id: event.metadata.get("tool_call_id")?.to_string(),
        tool_name: event.metadata.get("tool")?.to_string(),
        status: event.metadata.get("status")?.to_string(),
        output: event
            .metadata
            .get("output")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        timestamp_ms: event.timestamp_ms,
    })
}

pub(crate) fn tool_approval_from_audit(record: PermissionAuditRecord) -> Option<ToolApprovalView> {
    let can_allow_session = permission_can_allow_session(&record.request);
    Some(ToolApprovalView {
        request_id: record.request.id.0,
        invocation_id: record.request.metadata.get("tool_call_id")?.to_string(),
        tool_name: record.request.metadata.get("tool_name")?.to_string(),
        risk: permission_risk_label(&record.request.risk).to_string(),
        reason: redact_sensitive_text(&record.request.reason),
        scope: redact_sensitive_text(&record.request.scope),
        input: record
            .request
            .metadata
            .get("tool_input")
            .map(|value| redact_sensitive_text(value))
            .unwrap_or_default(),
        requested_at_ms: record.requested_at_ms,
        can_allow_session,
    })
}