use super::{AgentAttachmentView, AgentPolicy};
use agent_core::Event;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueuedAgentMessageView {
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) prompt: String,
    pub(crate) attachments: Vec<AgentAttachmentView>,
    pub(crate) effort: String,
    pub(crate) mode: String,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueuedAgentMessageReceipt {
    pub(crate) message: QueuedAgentMessageView,
    pub(crate) event_count: u64,
    pub(crate) latest_sequence: u64,
    pub(crate) latest_timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueuedAgentMessageActionReceipt {
    pub(crate) queue_id: String,
    pub(crate) message: Option<QueuedAgentMessageView>,
    pub(crate) event_count: u64,
    pub(crate) latest_sequence: u64,
    pub(crate) latest_timestamp_ms: u64,
    pub(crate) cancelled_active_run: bool,
    pub(crate) steer_committed: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueuedAgentMessagePayload {
    pub(crate) prompt: String,
    pub(crate) attachments: Vec<AgentAttachmentView>,
    pub(crate) effort: String,
    pub(crate) current_time: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingQueuedAgentMessage {
    pub(crate) view: QueuedAgentMessageView,
    pub(crate) payload: QueuedAgentMessagePayload,
    pub(crate) priority_sequence: u64,
}

pub(crate) fn is_agent_queue_event(event: &Event) -> bool {
    event.metadata.contains_key("queue_action") && event.metadata.contains_key("queue_id")
}

fn queued_message_payload(event: &Event) -> Option<QueuedAgentMessagePayload> {
    event
        .metadata
        .get("queue_payload")
        .and_then(|payload| serde_json::from_str(payload).ok())
}

fn sort_queued_agent_message_views(messages: &mut [QueuedAgentMessageView]) {
    messages.sort_by(|left, right| {
        match (
            left.mode.as_str() == "steer",
            right.mode.as_str() == "steer",
        ) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.id.cmp(&right.id)),
            (false, false) => left
                .created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.cmp(&right.id)),
        }
    });
}

fn apply_queue_event_to_views(messages: &mut Vec<QueuedAgentMessageView>, event: &Event) {
    let Some(action) = event.metadata.get("queue_action").map(String::as_str) else {
        return;
    };
    let Some(queue_id) = event.metadata.get("queue_id").cloned() else {
        return;
    };
    match action {
        "enqueue" | "restore" => {
            let Some(payload) = queued_message_payload(event) else {
                return;
            };
            let session_id = event
                .metadata
                .get("session_id")
                .cloned()
                .unwrap_or_default();
            let created_at_ms = event
                .metadata
                .get("queue_created_at_ms")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(event.timestamp_ms);
            let mode = event
                .metadata
                .get("queue_mode")
                .filter(|mode| mode.as_str() == "steer")
                .cloned()
                .unwrap_or_else(|| "queue".to_string());
            messages.retain(|message| message.id != queue_id);
            messages.push(QueuedAgentMessageView {
                id: queue_id,
                session_id,
                prompt: payload.prompt,
                attachments: payload.attachments,
                effort: AgentPolicy::parse_ingress(&payload.effort)
                    .label()
                    .to_string(),
                mode,
                created_at_ms,
                updated_at_ms: event.timestamp_ms,
            });
        }
        "edit" => {
            let Some(payload) = queued_message_payload(event) else {
                return;
            };
            if let Some(message) = messages.iter_mut().find(|message| message.id == queue_id) {
                message.prompt = payload.prompt;
                message.attachments = payload.attachments;
                message.effort = AgentPolicy::parse_ingress(&payload.effort)
                    .label()
                    .to_string();
                message.updated_at_ms = event.timestamp_ms;
            }
        }
        "steer" => {
            if let Some(message) = messages.iter_mut().find(|message| message.id == queue_id) {
                message.mode = "steer".to_string();
                message.updated_at_ms = event.timestamp_ms;
            }
        }
        "delete" | "start" => messages.retain(|message| message.id != queue_id),
        _ => {}
    }
    sort_queued_agent_message_views(messages);
}

fn apply_queue_event_to_payloads(
    payloads: &mut BTreeMap<String, QueuedAgentMessagePayload>,
    event: &Event,
) {
    let Some(action) = event.metadata.get("queue_action").map(String::as_str) else {
        return;
    };
    let Some(queue_id) = event.metadata.get("queue_id").cloned() else {
        return;
    };
    match action {
        "enqueue" | "restore" | "edit" => {
            if let Some(payload) = queued_message_payload(event) {
                payloads.insert(queue_id, payload);
            }
        }
        "delete" | "start" => {
            payloads.remove(&queue_id);
        }
        _ => {}
    }
}

pub(crate) fn apply_queue_event(
    messages: &mut Vec<QueuedAgentMessageView>,
    payloads: &mut BTreeMap<String, QueuedAgentMessagePayload>,
    event: &Event,
) {
    apply_queue_event_to_views(messages, event);
    apply_queue_event_to_payloads(payloads, event);
}

pub(crate) fn pending_queued_agent_messages(
    events: &[Event],
    session_id: &str,
) -> Vec<PendingQueuedAgentMessage> {
    let mut pending = BTreeMap::<String, PendingQueuedAgentMessage>::new();
    for event in events {
        if event.metadata.get("session_id").map(String::as_str) != Some(session_id) {
            continue;
        }
        let Some(action) = event.metadata.get("queue_action").map(String::as_str) else {
            continue;
        };
        let Some(queue_id) = event.metadata.get("queue_id").cloned() else {
            continue;
        };
        match action {
            "enqueue" | "restore" => {
                let Some(payload) = queued_message_payload(event) else {
                    continue;
                };
                let created_at_ms = event
                    .metadata
                    .get("queue_created_at_ms")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(event.timestamp_ms);
                let mode = event
                    .metadata
                    .get("queue_mode")
                    .filter(|mode| mode.as_str() == "steer")
                    .cloned()
                    .unwrap_or_else(|| "queue".to_string());
                pending.insert(
                    queue_id.clone(),
                    PendingQueuedAgentMessage {
                        view: QueuedAgentMessageView {
                            id: queue_id,
                            session_id: session_id.to_string(),
                            prompt: payload.prompt.clone(),
                            attachments: payload.attachments.clone(),
                            effort: AgentPolicy::parse_ingress(&payload.effort)
                                .label()
                                .to_string(),
                            mode,
                            created_at_ms,
                            updated_at_ms: event.timestamp_ms,
                        },
                        payload,
                        priority_sequence: event.sequence,
                    },
                );
            }
            "edit" => {
                let Some(payload) = queued_message_payload(event) else {
                    continue;
                };
                if let Some(message) = pending.get_mut(&queue_id) {
                    message.view.prompt = payload.prompt.clone();
                    message.view.attachments = payload.attachments.clone();
                    message.view.effort = AgentPolicy::parse_ingress(&payload.effort)
                        .label()
                        .to_string();
                    message.view.updated_at_ms = event.timestamp_ms;
                    message.payload = payload;
                }
            }
            "steer" => {
                if let Some(message) = pending.get_mut(&queue_id) {
                    message.view.mode = "steer".to_string();
                    message.view.updated_at_ms = event.timestamp_ms;
                    message.priority_sequence = event.sequence;
                }
            }
            "delete" | "start" => {
                pending.remove(&queue_id);
            }
            _ => {}
        }
    }
    let mut messages = pending.into_values().collect::<Vec<_>>();
    messages.sort_by(|left, right| {
        match (
            left.view.mode.as_str() == "steer",
            right.view.mode.as_str() == "steer",
        ) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => right
                .priority_sequence
                .cmp(&left.priority_sequence)
                .then_with(|| left.view.id.cmp(&right.view.id)),
            (false, false) => left
                .view
                .created_at_ms
                .cmp(&right.view.created_at_ms)
                .then_with(|| left.view.id.cmp(&right.view.id)),
        }
    });
    messages
}

pub(crate) fn queued_agent_message_id(
    candidate: Option<&str>,
    fallback: impl FnOnce() -> String,
) -> String {
    candidate
        .map(str::trim)
        .filter(|value| {
            value.starts_with("agent-queue-client-")
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .map(str::to_string)
        .unwrap_or_else(fallback)
}
